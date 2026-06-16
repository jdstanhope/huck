//! `complete`, `compgen`, `compopt` builtins. Flag parsing produces a
//! `CompletionSpec`; storage and resolution are delegated to the
//! `completion_spec` module.

use std::io::Write;
use std::rc::Rc;

use crate::builtins::ExecOutcome;
use crate::completion_spec::{Action, CompOptions, CompletionCtx, CompletionSpec};
use crate::shell_state::Shell;

/// Output of parsing a `complete` / `compgen` flag string.
#[derive(Debug, Default)]
struct ParsedFlags {
    spec: CompletionSpec,
    /// -D: apply to default (no other spec matched).
    is_default: bool,
    /// -E: apply when completing on empty command line.
    is_empty: bool,
    /// -p: print mode.
    print: bool,
    /// -r: remove mode.
    remove: bool,
    /// True if `-o`/`+o` was used. Tracked separately from `spec` because
    /// `+o NAME` may set an option to its default value (false) — leaving
    /// `spec == CompletionSpec::default()` — yet still represent an
    /// intentional mutation that should bypass the "nothing to complete"
    /// guard in register mode.
    options_touched: bool,
    /// Trailing positional args (command names for `complete`, optional
    /// word arg for `compgen`).
    positional: Vec<String>,
}

#[derive(Debug)]
enum FlagError {
    Usage(String),
    InvalidAction(String),
    InvalidOption(String),
    MissingArg(char),
}

impl FlagError {
    fn diag(&self, cmd: &str) -> String {
        match self {
            FlagError::Usage(msg) => format!("huck: {cmd}: {msg}"),
            FlagError::InvalidAction(name) => {
                format!("huck: {cmd}: {name}: invalid action name")
            }
            FlagError::InvalidOption(name) => {
                format!("huck: {cmd}: {name}: invalid completion option")
            }
            FlagError::MissingArg(c) => {
                format!("huck: {cmd}: -{c}: option requires an argument")
            }
        }
    }

    fn status(&self) -> i32 {
        match self {
            FlagError::Usage(_) => 2,
            _ => 2,
        }
    }
}

/// Parses the flags. `allow_d_e` controls whether `-D`/`-E`/`-p`/`-r`
/// are accepted (true for `complete`, false for `compgen`).
fn parse_flags(args: &[String], allow_d_e: bool) -> Result<ParsedFlags, FlagError> {
    let mut out = ParsedFlags::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') && !arg.starts_with('+') {
            break;
        }
        if arg == "-" || arg == "+" {
            return Err(FlagError::Usage(format!("bad option: {arg}")));
        }
        // Cluster: each character after the leading -/+ is a flag.
        // Flags that take an arg consume the remainder of the current
        // word (inline) OR the next word.
        let leading = arg.chars().next().unwrap(); // - or +
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut ci = 0;
        while ci < chars.len() {
            let c = chars[ci];
            match c {
                'F' | 'W' | 'G' | 'A' | 'P' | 'S' | 'X' | 'o' => {
                    // Only F/W/G/A/P/S/X reject `+`. `'o'` accepts `+` so
                    // `complete +o NAME` can clear a previously-set option.
                    if leading == '+' && c != 'o' {
                        return Err(FlagError::Usage(format!("+{c}: not supported")));
                    }
                    // Argument is either the rest of this word or the next word.
                    let arg_value: String = if ci + 1 < chars.len() {
                        let v: String = chars[ci + 1..].iter().collect();
                        ci = chars.len(); // consume rest of this word
                        v
                    } else if i + 1 < args.len() {
                        i += 1;
                        ci = chars.len();
                        args[i].clone()
                    } else {
                        return Err(FlagError::MissingArg(c));
                    };
                    match c {
                        'F' => out.spec.function = Some(arg_value),
                        'W' => out.spec.wordlist = Some(arg_value),
                        'G' => out.spec.glob = Some(arg_value),
                        'A' => {
                            let action = Action::parse(&arg_value)
                                .ok_or_else(|| FlagError::InvalidAction(arg_value.clone()))?;
                            out.spec.actions.push(action);
                        }
                        'P' => out.spec.prefix = Some(arg_value),
                        'S' => out.spec.suffix = Some(arg_value),
                        'X' => out.spec.filter = Some(arg_value),
                        'o' => {
                            apply_option(&mut out.spec.options, &arg_value, leading == '+')?;
                            out.options_touched = true;
                        }
                        _ => unreachable!(),
                    }
                }
                'D' if allow_d_e => out.is_default = true,
                'E' if allow_d_e => out.is_empty = true,
                'p' if allow_d_e => out.print = true,
                'r' if allow_d_e => out.remove = true,
                'a' | 'b' | 'c' | 'd' | 'e' | 'f' | 'g' | 'j' | 'k' | 's' | 'u' | 'v' => {
                    if leading == '+' {
                        return Err(FlagError::Usage(format!("+{c}: not supported")));
                    }
                    let action = match c {
                        'a' => Action::Alias,
                        'b' => Action::Builtin,
                        'c' => Action::Command,
                        'd' => Action::Directory,
                        'e' => Action::Export,
                        'f' => Action::File,
                        'g' => Action::Group,
                        'j' => Action::Job,
                        'k' => Action::Keyword,
                        's' => Action::Service,
                        'u' => Action::User,
                        'v' => Action::Variable,
                        _ => unreachable!(),
                    };
                    out.spec.actions.push(action);
                }
                other => {
                    return Err(FlagError::Usage(format!("-{other}: invalid option")));
                }
            }
            ci += 1;
        }
        i += 1;
    }
    out.positional = args[i..].to_vec();
    Ok(out)
}

fn apply_option(opts: &mut CompOptions, name: &str, off: bool) -> Result<(), FlagError> {
    let value = !off;
    match name {
        "default" => opts.default = value,
        "nospace" => opts.nospace = value,
        "filenames" => opts.filenames = value,
        "bashdefault" => opts.bashdefault = value,
        "dirnames" => opts.dirnames = value,
        "nosort" => opts.nosort = value,
        "noquote" => opts.noquote = value,
        "plusdirs" => opts.plusdirs = value,
        _ => return Err(FlagError::InvalidOption(name.to_string())),
    }
    Ok(())
}

/// `complete` builtin.
pub fn builtin_complete(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_flags(args, true) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.diag("complete"));
            return ExecOutcome::Continue(e.status());
        }
    };

    // Mode: print
    if parsed.print || is_bare(&parsed) {
        return print_complete(
            &parsed.positional,
            parsed.is_default,
            parsed.is_empty,
            out,
            shell,
        );
    }
    // Mode: remove
    if parsed.remove {
        return remove_complete(&parsed.positional, &parsed, shell);
    }
    // Mode: register
    register_complete(&parsed, shell)
}

fn is_bare(parsed: &ParsedFlags) -> bool {
    let spec_empty = parsed.spec == CompletionSpec::default();
    spec_empty
        && !parsed.is_default
        && !parsed.is_empty
        && !parsed.remove
        && parsed.positional.is_empty()
}

fn print_complete(
    names: &[String],
    is_default: bool,
    is_empty: bool,
    out: &mut dyn Write,
    shell: &Shell,
) -> ExecOutcome {
    let specs = &shell.completion_specs;
    let mut status: i32 = 0;

    // -D / -E narrow the print to just the matching slot.
    if is_default {
        match &specs.default_spec {
            Some(d) => {
                let _ = writeln!(out, "{}", format_spec_for_print(d, None, Some("-D")));
            }
            None => {
                eprintln!("huck: complete: no completion specification for -D");
                status = 1;
            }
        }
    }
    if is_empty {
        match &specs.empty_spec {
            Some(e) => {
                let _ = writeln!(out, "{}", format_spec_for_print(e, None, Some("-E")));
            }
            None => {
                eprintln!("huck: complete: no completion specification for -E");
                status = 1;
            }
        }
    }
    if is_default || is_empty {
        return ExecOutcome::Continue(status);
    }

    if names.is_empty() {
        // Print all in sorted order: by_command first, then -D, then -E.
        let mut keys: Vec<&String> = specs.by_command.keys().collect();
        keys.sort();
        for k in keys {
            let _ = writeln!(
                out,
                "{}",
                format_spec_for_print(&specs.by_command[k], Some(k.as_str()), None)
            );
        }
        if let Some(d) = &specs.default_spec {
            let _ = writeln!(out, "{}", format_spec_for_print(d, None, Some("-D")));
        }
        if let Some(e) = &specs.empty_spec {
            let _ = writeln!(out, "{}", format_spec_for_print(e, None, Some("-E")));
        }
    } else {
        for n in names {
            match specs.by_command.get(n) {
                Some(s) => {
                    let _ = writeln!(out, "{}", format_spec_for_print(s, Some(n.as_str()), None));
                }
                None => {
                    eprintln!("huck: complete: {n}: no completion specification");
                    status = 1;
                }
            }
        }
    }
    ExecOutcome::Continue(status)
}

fn remove_complete(names: &[String], parsed: &ParsedFlags, shell: &mut Shell) -> ExecOutcome {
    let mut status = 0;
    let specs = Rc::make_mut(&mut shell.completion_specs);
    if parsed.is_default {
        specs.default_spec = None;
    }
    if parsed.is_empty {
        specs.empty_spec = None;
    }
    if names.is_empty() && !parsed.is_default && !parsed.is_empty {
        specs.by_command.clear();
    } else {
        for n in names {
            if specs.by_command.remove(n).is_none()
                && !parsed.is_default
                && !parsed.is_empty
            {
                eprintln!("huck: complete: {n}: no completion specification");
                status = 1;
            }
        }
    }
    ExecOutcome::Continue(status)
}

fn register_complete(parsed: &ParsedFlags, shell: &mut Shell) -> ExecOutcome {
    if (parsed.is_default || parsed.is_empty) && !parsed.positional.is_empty() {
        eprintln!("huck: complete: cannot use -D or -E with command names");
        return ExecOutcome::Continue(2);
    }
    if !parsed.positional.is_empty()
        && parsed.spec == CompletionSpec::default()
        && !parsed.options_touched
    {
        eprintln!("huck: complete: nothing to complete");
        return ExecOutcome::Continue(1);
    }
    let specs = Rc::make_mut(&mut shell.completion_specs);
    if parsed.is_default {
        specs.default_spec = Some(parsed.spec.clone());
    }
    if parsed.is_empty {
        specs.empty_spec = Some(parsed.spec.clone());
    }
    for n in &parsed.positional {
        specs.by_command.insert(n.clone(), parsed.spec.clone());
    }
    ExecOutcome::Continue(0)
}

/// Renders a spec for `complete -p` in deterministic re-input form.
fn format_spec_for_print(
    spec: &CompletionSpec,
    name: Option<&str>,
    mode: Option<&str>,
) -> String {
    let mut parts: Vec<String> = vec!["complete".to_string()];
    if let Some(m) = mode {
        parts.push(m.to_string());
    }
    if let Some(f) = &spec.function {
        parts.push(format!("-F '{}'", crate::builtins::escape_alias_value(f)));
    }
    if let Some(w) = &spec.wordlist {
        parts.push(format!("-W '{}'", crate::builtins::escape_alias_value(w)));
    }
    if let Some(g) = &spec.glob {
        parts.push(format!("-G '{}'", crate::builtins::escape_alias_value(g)));
    }
    for a in &spec.actions {
        parts.push(format!("-A {}", a.as_str()));
    }
    if let Some(p) = &spec.prefix {
        parts.push(format!("-P '{}'", crate::builtins::escape_alias_value(p)));
    }
    if let Some(s) = &spec.suffix {
        parts.push(format!("-S '{}'", crate::builtins::escape_alias_value(s)));
    }
    if let Some(x) = &spec.filter {
        parts.push(format!("-X '{}'", crate::builtins::escape_alias_value(x)));
    }
    let CompOptions {
        default,
        nospace,
        filenames,
        bashdefault,
        dirnames,
        nosort,
        noquote,
        plusdirs,
    } = spec.options;
    if default {
        parts.push("-o default".to_string());
    }
    if nospace {
        parts.push("-o nospace".to_string());
    }
    if filenames {
        parts.push("-o filenames".to_string());
    }
    if bashdefault {
        parts.push("-o bashdefault".to_string());
    }
    if dirnames {
        parts.push("-o dirnames".to_string());
    }
    if noquote {
        parts.push("-o noquote".to_string());
    }
    if nosort {
        parts.push("-o nosort".to_string());
    }
    if plusdirs {
        parts.push("-o plusdirs".to_string());
    }
    if let Some(n) = name {
        parts.push("--".to_string());
        parts.push(n.to_string());
    }
    parts.join(" ")
}

/// `compgen` builtin.
pub fn builtin_compgen(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_flags(args, false) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.diag("compgen"));
            return ExecOutcome::Continue(e.status());
        }
    };

    let word = parsed.positional.first().cloned().unwrap_or_default();
    let ctx = CompletionCtx {
        cmd_name: "compgen".to_string(),
        cur_word: word.clone(),
        prev_word: String::new(),
        comp_words: vec![word.clone()],
        comp_cword: 0,
        comp_line: word.clone(),
        comp_point: word.len(),
    };
    // Save+restore shell.current_completion_spec around run_spec.
    // run_spec with a -F function calls call_completion_function,
    // which INTENTIONALLY leaves the synthetic compgen spec stashed in
    // current_completion_spec (so Task-5 dispatch can read compopt-applied
    // mutations). For `compgen` from script context we have no consumer,
    // so leaving it set would leak: the NEXT tab dispatch on an unrelated
    // spec would .take() the leftover compgen spec (with all-default
    // options) and silently override the real spec's options. Snapshotting
    // around the call keeps the slot's contents unchanged for callers
    // (e.g., a -F dispatcher that internally calls `compgen -F _other`
    // must see ITS spec on return, not _other's).
    let saved = shell.current_completion_spec.take();
    let results = crate::completion_spec::run_spec(&parsed.spec, &ctx, shell);
    shell.current_completion_spec = saved;
    let any = !results.is_empty();
    for r in results {
        let _ = writeln!(out, "{r}");
    }
    ExecOutcome::Continue(if any { 0 } else { 1 })
}

/// `compopt` builtin. Two modes:
///
/// * In-function (no names): mutates the live spec via
///   `shell.current_completion_spec`, which the Task-5 dispatch path
///   takes back out after the `-F` function returns. Errors with status
///   1 when called outside a `-F` function with no names.
///
/// * Named (with names): mutates `shell.completion_specs.by_command[name]`
///   directly. `-o` sets, `+o` clears. Status 1 if any name is missing.
///
/// `-D` / `-E` (mutate default/empty specs from within a function) are
/// recognized as flags but rejected with status 2 (parse-time error,
/// like any other unsupported flag) — "not yet supported".
pub fn builtin_compopt(args: &[String], _out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let mut i = 0;
    let mut option_set: Vec<(String, bool)> = Vec::new();
    let mut is_default = false;
    let mut is_empty = false;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') && !arg.starts_with('+') {
            break;
        }
        if arg == "-" || arg == "+" {
            eprintln!("huck: compopt: bad option: {arg}");
            return ExecOutcome::Continue(2);
        }
        let leading = arg.chars().next().unwrap();
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut ci = 0;
        while ci < chars.len() {
            let c = chars[ci];
            match c {
                'o' => {
                    let arg_value: String = if ci + 1 < chars.len() {
                        let v: String = chars[ci + 1..].iter().collect();
                        ci = chars.len();
                        v
                    } else if i + 1 < args.len() {
                        i += 1;
                        ci = chars.len();
                        args[i].clone()
                    } else {
                        eprintln!("huck: compopt: -o: option requires an argument");
                        return ExecOutcome::Continue(2);
                    };
                    let off = leading == '+';
                    if !["default", "nospace", "filenames", "bashdefault", "dirnames"]
                        .contains(&arg_value.as_str())
                    {
                        eprintln!("huck: compopt: {arg_value}: invalid completion option");
                        return ExecOutcome::Continue(2);
                    }
                    option_set.push((arg_value, off));
                }
                'D' => {
                    is_default = true;
                }
                'E' => {
                    is_empty = true;
                }
                other => {
                    eprintln!("huck: compopt: -{other}: invalid option");
                    return ExecOutcome::Continue(2);
                }
            }
            ci += 1;
        }
        i += 1;
    }
    let names: Vec<String> = args[i..].to_vec();

    if is_default || is_empty {
        eprintln!("huck: compopt: -D/-E not yet supported");
        return ExecOutcome::Continue(2);
    }

    if names.is_empty() {
        // In-function mutation. The dispatch path stashes the live spec
        // in shell.current_completion_spec before invoking -F; we take
        // it out, mutate, and put it back so dispatch's later .take()
        // observes the change.
        let Some(mut live) = shell.current_completion_spec.take() else {
            eprintln!("huck: compopt: not currently executing completion function");
            return ExecOutcome::Continue(1);
        };
        apply_compopt_options(&mut live.options, &option_set);
        shell.current_completion_spec = Some(live);
        return ExecOutcome::Continue(0);
    }

    // Named: mutate registry.
    let mut status = 0;
    for n in &names {
        match Rc::make_mut(&mut shell.completion_specs).by_command.get_mut(n) {
            Some(spec) => apply_compopt_options(&mut spec.options, &option_set),
            None => {
                eprintln!("huck: compopt: {n}: no completion specification");
                status = 1;
            }
        }
    }
    ExecOutcome::Continue(status)
}

/// Applies a list of (name, off) compopt option mutations to a CompOptions.
/// The option names have already been validated against the whitelist.
fn apply_compopt_options(opts: &mut CompOptions, sets: &[(String, bool)]) {
    for (name, off) in sets {
        let v = !*off;
        match name.as_str() {
            "default" => opts.default = v,
            "nospace" => opts.nospace = v,
            "filenames" => opts.filenames = v,
            "bashdefault" => opts.bashdefault = v,
            "dirnames" => opts.dirnames = v,
            _ => unreachable!("name pre-validated by builtin_compopt"),
        }
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run_complete(args: &[&str], shell: &mut Shell) -> (String, i32) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::<u8>::new();
        let outcome = builtin_complete(&argv, &mut out, shell);
        let s = String::from_utf8(out).unwrap();
        let code = match outcome {
            ExecOutcome::Continue(n) => n,
            _ => panic!("complete should not return non-Continue"),
        };
        (s, code)
    }

    fn run_compgen(args: &[&str], shell: &mut Shell) -> (String, i32) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::<u8>::new();
        let outcome = builtin_compgen(&argv, &mut out, shell);
        let s = String::from_utf8(out).unwrap();
        let code = match outcome {
            ExecOutcome::Continue(n) => n,
            _ => panic!("compgen should not return non-Continue"),
        };
        (s, code)
    }

    fn run_compopt(args: &[&str], shell: &mut Shell) -> (String, i32) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::<u8>::new();
        let outcome = builtin_compopt(&argv, &mut out, shell);
        let s = String::from_utf8(out).unwrap();
        let code = match outcome {
            ExecOutcome::Continue(n) => n,
            _ => panic!("compopt should not return non-Continue"),
        };
        (s, code)
    }

    #[test]
    fn complete_registers_and_prints() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-W", "alpha alpine beta", "--", "myc"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command.contains_key("myc"));
        let spec = &sh.completion_specs.by_command["myc"];
        assert_eq!(spec.wordlist, Some("alpha alpine beta".to_string()));

        let (out, code) = run_complete(&["-p", "myc"], &mut sh);
        assert_eq!(code, 0);
        assert!(out.contains("complete"));
        assert!(out.contains("-W"));
        assert!(out.contains("alpha alpine beta"));
        assert!(out.contains("myc"));
    }

    #[test]
    fn complete_unknown_name_for_p_returns_1() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-p", "nope"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn complete_r_removes_spec() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "foo"], &mut sh);
        assert!(sh.completion_specs.by_command.contains_key("foo"));
        let (_, code) = run_complete(&["-r", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert!(!sh.completion_specs.by_command.contains_key("foo"));
    }

    #[test]
    fn complete_r_missing_name_returns_1() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-r", "ghost"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn complete_r_bare_clears_all_by_command() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "a"], &mut sh);
        let _ = run_complete(&["-W", "y", "--", "b"], &mut sh);
        let (_, code) = run_complete(&["-r"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command.is_empty());
    }

    #[test]
    fn complete_D_sets_default_spec() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-D", "-W", "fallback"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.default_spec.is_some());
    }

    #[test]
    fn complete_D_with_names_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-D", "-W", "x", "--", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn complete_invalid_action_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-A", "bogus_action", "--", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn complete_invalid_option_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-o", "bogus", "--", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn complete_nosort_noquote_plusdirs_accepted() {
        // These three `-o` options are accepted (previously rejected). They
        // install into CompOptions and the compspec registers successfully.
        let mut sh = Shell::new();
        let (_, code) = run_complete(
            &["-o", "nosort", "-o", "noquote", "-o", "plusdirs", "-W", "x", "--", "foo"],
            &mut sh,
        );
        assert_eq!(code, 0);
        let opts = sh.completion_specs.by_command["foo"].options;
        assert!(opts.nosort && opts.noquote && opts.plusdirs);
    }

    #[test]
    fn complete_plus_o_nosort_clears_it() {
        let mut sh = Shell::new();
        let (_, c1) = run_complete(&["-o", "nosort", "-W", "x", "--", "foo"], &mut sh);
        assert_eq!(c1, 0);
        let (_, c2) = run_complete(&["+o", "nosort", "--", "foo"], &mut sh);
        assert_eq!(c2, 0);
        assert!(!sh.completion_specs.by_command["foo"].options.nosort);
    }

    #[test]
    fn complete_inline_flag_arg() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-Falpha", "--", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert_eq!(
            sh.completion_specs.by_command["foo"].function,
            Some("alpha".to_string())
        );
    }

    #[test]
    fn complete_nothing_to_complete_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["foo"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn compgen_W_filters_by_prefix_arg() {
        let mut sh = Shell::new();
        let (out, code) = run_compgen(&["-W", "alpha alpine beta", "al"], &mut sh);
        assert_eq!(code, 0);
        assert_eq!(out, "alpha\nalpine\n");
    }

    #[test]
    fn compgen_no_match_returns_1() {
        let mut sh = Shell::new();
        let (out, code) = run_compgen(&["-W", "a b c", "z"], &mut sh);
        assert_eq!(code, 1);
        assert_eq!(out, "");
    }

    #[test]
    fn compgen_A_builtin() {
        let mut sh = Shell::new();
        let (out, code) = run_compgen(&["-A", "builtin", "ec"], &mut sh);
        assert_eq!(code, 0);
        assert!(out.contains("echo"));
    }

    #[test]
    fn complete_multiple_actions_accumulate() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-A", "builtin", "-A", "keyword", "--", "foo"], &mut sh);
        assert_eq!(code, 0);
        let acts = &sh.completion_specs.by_command["foo"].actions;
        assert_eq!(acts.len(), 2);
        assert!(acts.contains(&Action::Builtin));
        assert!(acts.contains(&Action::Keyword));
    }

    #[test]
    fn complete_print_form_round_trips_wordlist() {
        let mut sh = Shell::new();
        let _ = run_complete(
            &["-W", "alpha apple banana", "-P", "x:", "--", "myc"],
            &mut sh,
        );
        let (out, _) = run_complete(&["-p", "myc"], &mut sh);

        // Tokenize the print output. Output is one line like:
        // `complete -W 'alpha apple banana' -P 'x:' -- myc`
        let tokens = tokenize_posix_line(out.trim_end());
        assert_eq!(tokens[0], "complete");

        let parsed = super::parse_flags(&tokens[1..], true).expect("re-parse");
        let original = &sh.completion_specs.by_command["myc"];
        assert_eq!(&parsed.spec, original, "round-trip mismatch");
        assert_eq!(parsed.positional, vec!["myc".to_string()]);
    }

    /// Splits a string into POSIX-style tokens, honoring single-quote
    /// strings. Outside single quotes, whitespace separates tokens. Inside
    /// single quotes, every character (including spaces) is literal; a
    /// closing single quote ends the quoted segment. POSIX `'\''` is the
    /// way to embed a single quote, but `format_spec_for_print` does not
    /// emit literal single quotes (it relies on `escape_alias_value` which
    /// is fine for the round-trip cases we test here).
    fn tokenize_posix_line(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_single = false;
        let mut started = false; // current token has begun (allows empty '')
        for c in line.chars() {
            if in_single {
                if c == '\'' {
                    in_single = false;
                } else {
                    cur.push(c);
                }
                continue;
            }
            if c == '\'' {
                in_single = true;
                started = true;
                continue;
            }
            if c.is_ascii_whitespace() {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
                continue;
            }
            cur.push(c);
            started = true;
        }
        if started {
            out.push(cur);
        }
        out
    }

    #[test]
    fn complete_multi_name_registration() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-W", "x y z", "--", "foo", "bar", "baz"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command.contains_key("foo"));
        assert!(sh.completion_specs.by_command.contains_key("bar"));
        assert!(sh.completion_specs.by_command.contains_key("baz"));
        // All three specs should be equal (the same spec was cloned per name).
        assert_eq!(
            sh.completion_specs.by_command["foo"],
            sh.completion_specs.by_command["bar"]
        );
        assert_eq!(
            sh.completion_specs.by_command["foo"],
            sh.completion_specs.by_command["baz"]
        );
    }

    #[test]
    fn complete_plus_o_clears_option() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(
            &["-W", "x", "-o", "nospace", "--", "foo"],
            &mut sh,
        );
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command["foo"].options.nospace);

        let (_, code) = run_complete(&["+o", "nospace", "--", "foo"], &mut sh);
        assert_eq!(code, 0, "complete +o should be accepted");
        assert!(!sh.completion_specs.by_command["foo"].options.nospace);
    }

    #[test]
    fn compopt_outside_function_with_no_name_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_compopt(&["-o", "nospace"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn compopt_named_mutates_registry() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "foo"], &mut sh);
        let (_, code) = run_compopt(&["-o", "nospace", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command["foo"].options.nospace);
    }

    #[test]
    fn compopt_named_plus_o_unsets() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "-o", "nospace", "--", "foo"], &mut sh);
        assert!(sh.completion_specs.by_command["foo"].options.nospace);
        let (_, code) = run_compopt(&["+o", "nospace", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert!(!sh.completion_specs.by_command["foo"].options.nospace);
    }

    #[test]
    fn compopt_named_missing_returns_1() {
        let mut sh = Shell::new();
        let (_, code) = run_compopt(&["-o", "nospace", "ghost"], &mut sh);
        assert_eq!(code, 1);
    }

    #[test]
    fn compopt_invalid_option_errors() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "foo"], &mut sh);
        let (_, code) = run_compopt(&["-o", "nosort", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn compopt_in_function_mutates_live_spec() {
        let mut sh = Shell::new();
        // Function calls `compopt -o nospace` then sets COMPREPLY.
        let _ = crate::shell::process_line(
            "_myf() { compopt -o nospace; COMPREPLY=(alpha); }",
            &mut sh,
            false,
        );

        let spec = crate::completion_spec::CompletionSpec {
            function: Some("_myf".to_string()),
            ..Default::default()
        };
        let ctx = crate::completion_spec::CompletionCtx {
            cmd_name: "myc".to_string(),
            cur_word: String::new(),
            prev_word: String::new(),
            comp_words: vec!["myc".to_string(), String::new()],
            comp_cword: 1,
            comp_line: "myc ".to_string(),
            comp_point: 4,
        };
        let _ = crate::completion_spec::run_spec(&spec, &ctx, &mut sh);
        // After run_spec, dispatch reads current_completion_spec —
        // but for this unit test we read it directly to verify the
        // function's compopt call mutated it.
        let mutated = sh
            .current_completion_spec
            .as_ref()
            .expect("spec still stashed after -F returns");
        assert!(
            mutated.options.nospace,
            "compopt -o nospace inside -F did not take effect"
        );
    }

    #[test]
    fn complete_p_with_D_prints_only_default() {
        let mut sh = Shell::new();
        let _ = run_complete(&["-W", "x", "--", "foo"], &mut sh);
        let _ = run_complete(&["-D", "-F", "_default_func"], &mut sh);

        let (out, code) = run_complete(&["-p", "-D"], &mut sh);
        assert_eq!(code, 0);
        // -D output should mention -D and _default_func.
        assert!(out.contains("-D"), "{out:?}");
        assert!(out.contains("_default_func"), "{out:?}");
        // Should NOT contain the by_command entry's name "foo".
        assert!(!out.contains(" -- foo"), "should not print foo's spec: {out:?}");
    }

    #[test]
    fn compopt_D_rejected_with_exit_2() {
        let mut sh = Shell::new();
        let (_, code) = run_compopt(&["-D", "-o", "nospace"], &mut sh);
        assert_eq!(code, 2, "compopt -D is a parse-time rejection, should be exit 2");
    }

    #[test]
    fn compopt_E_rejected_with_exit_2() {
        let mut sh = Shell::new();
        let (_, code) = run_compopt(&["-E", "-o", "nospace"], &mut sh);
        assert_eq!(code, 2, "compopt -E is a parse-time rejection, should be exit 2");
    }

    #[test]
    fn compgen_F_does_not_leak_current_completion_spec() {
        let mut sh = Shell::new();
        // Define a function and run compgen -F. After it returns,
        // shell.current_completion_spec MUST be None — otherwise the
        // next tab dispatch on an unrelated spec gets the wrong options.
        let _ = crate::shell::process_line(
            "_myf() { COMPREPLY=(a b); }",
            &mut sh,
            false,
        );
        let _ = run_compgen(&["-F", "_myf"], &mut sh);
        assert!(
            sh.current_completion_spec.is_none(),
            "compgen -F leaked current_completion_spec across the call: \
             {:?}",
            sh.current_completion_spec,
        );
    }

    #[test]
    fn compopt_double_dash_ends_flags() {
        let mut sh = Shell::new();
        // After --, "foo" should be a name (not a flag). With no registered
        // spec for "foo", this errors with exit 1 (missing name).
        let (_, code) = run_compopt(&["-o", "nospace", "--", "foo"], &mut sh);
        assert_eq!(code, 1, "-- should end flags so 'foo' is a name; no spec → exit 1");
    }

    #[test]
    fn complete_short_flag_actions_map_to_actions() {
        use crate::completion_spec::Action;
        let cases = [
            ("-a", Action::Alias), ("-b", Action::Builtin), ("-c", Action::Command),
            ("-d", Action::Directory), ("-e", Action::Export), ("-f", Action::File),
            ("-g", Action::Group), ("-j", Action::Job), ("-k", Action::Keyword),
            ("-s", Action::Service), ("-u", Action::User), ("-v", Action::Variable),
        ];
        for (flag, want) in cases {
            let mut sh = Shell::new();
            let (_, code) = run_complete(&[flag, "--", "foo"], &mut sh);
            assert_eq!(code, 0, "flag {flag} should be accepted");
            assert_eq!(sh.completion_specs.by_command["foo"].actions, vec![want],
                "flag {flag} → wrong action");
        }
    }

    #[test]
    fn complete_clustered_short_flags_accumulate() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-ev", "--", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert_eq!(sh.completion_specs.by_command["foo"].actions,
            vec![crate::completion_spec::Action::Export, crate::completion_spec::Action::Variable]);
    }
}
