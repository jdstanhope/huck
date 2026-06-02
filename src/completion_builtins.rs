//! `complete`, `compgen`, `compopt` builtins. Flag parsing produces a
//! `CompletionSpec`; storage and resolution are delegated to the
//! `completion_spec` module.

use std::io::Write;

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
                    if leading == '+' {
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
                        'o' => apply_option(&mut out.spec.options, &arg_value, leading == '+')?,
                        _ => unreachable!(),
                    }
                }
                'D' if allow_d_e => out.is_default = true,
                'E' if allow_d_e => out.is_empty = true,
                'p' if allow_d_e => out.print = true,
                'r' if allow_d_e => out.remove = true,
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
        // Recognized-but-rejected: parse error per spec.
        "nosort" | "noquote" | "plusdirs" => {
            return Err(FlagError::InvalidOption(name.to_string()));
        }
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
        return print_complete(&parsed.positional, out, shell);
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

fn print_complete(names: &[String], out: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    let specs = &shell.completion_specs;
    let mut status: i32 = 0;
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
    let specs = &mut shell.completion_specs;
    let mut status = 0;
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
            if specs.by_command.remove(n).is_none() && !parsed.is_default && !parsed.is_empty {
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
    if !parsed.positional.is_empty() && parsed.spec == CompletionSpec::default() {
        eprintln!("huck: complete: nothing to complete");
        return ExecOutcome::Continue(1);
    }
    if parsed.is_default {
        shell.completion_specs.default_spec = Some(parsed.spec.clone());
    }
    if parsed.is_empty {
        shell.completion_specs.empty_spec = Some(parsed.spec.clone());
    }
    for n in &parsed.positional {
        shell
            .completion_specs
            .by_command
            .insert(n.clone(), parsed.spec.clone());
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
    let results = crate::completion_spec::resolve_spec(&parsed.spec, &ctx, shell);
    let any = !results.is_empty();
    for r in results {
        let _ = writeln!(out, "{r}");
    }
    ExecOutcome::Continue(if any { 0 } else { 1 })
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
        let (_, code) = run_complete(&["-A", "hostname", "--", "foo"], &mut sh);
        assert_eq!(code, 2);
    }

    #[test]
    fn complete_invalid_option_errors() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-o", "nosort", "--", "foo"], &mut sh);
        assert_eq!(code, 2);
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
        let _ = run_complete(&["-W", "a b c", "--", "myc"], &mut sh);
        let (out, _) = run_complete(&["-p", "myc"], &mut sh);
        // The output should be a complete-form line that, if re-parsed,
        // produces the same spec.
        assert!(out.starts_with("complete "));
        assert!(out.contains("-W 'a b c'") || out.contains("-W \"a b c\""));
        assert!(out.contains("-- myc"));
    }
}
