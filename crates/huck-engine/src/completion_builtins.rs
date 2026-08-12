//! `complete`, `compgen`, `compopt` builtins. Flag parsing produces a
//! `CompletionSpec`; storage and resolution are delegated to the
//! `completion_spec` module.

use std::io::Write;
use std::rc::Rc;

use crate::builtins::ExecOutcome;
use crate::completion_spec::{Action, CompKey, CompOptions, CompletionCtx, CompletionSpec};
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
    /// Whether the scanner consumed at least one OPTION (`-x`, `-W list`, …).
    /// A bare `--` does not count, and neither does a `+`-prefixed word (which
    /// is a positional here — #515). `compgen` needs this: bash's
    /// `build_actions` fails when no option was given and `compgen_builtin`
    /// turns that failure into success WITHOUT generating anything (#528).
    saw_option: bool,
}

/// Parses the flags. `allow_d_e` controls whether `-D`/`-E`/`-p`/`-r`
/// are accepted (true for `complete`, false for `compgen`). `name` is the
/// invoked name ("complete" or "compgen"), used both for the shared
/// scanner's diagnostics and this function's own.
///
/// Scanned entirely by the shared `Getopt` (#496); on `Err(code)` it has
/// ALREADY emitted both diagnostic lines, so this just propagates the code.
///
/// There is no `+` handling, and that is the point: bash's `complete` and
/// `compgen` do not parse `+` at all (#515). A `+`-prefixed argument is simply
/// the first non-option, so the scanner stops and it becomes a NAME — which is
/// exactly what bash does. huck previously ran a hand-rolled `+` loop here,
/// alternating with the scanner; it rejected `+z` as an invalid option and
/// swallowed `complete +o nospace foo`'s names.
fn parse_flags(
    args: &[String],
    allow_d_e: bool,
    name: &str,
    shell: &mut Shell,
    err: &mut dyn Write,
) -> Result<ParsedFlags, i32> {
    let mut out = ParsedFlags::default();
    // `I` (bash: `-I`/`+I`, "completion on the initial word") and `C`
    // (bash: `-C command`, generate completions by running a shell command)
    // are real, bash-implemented options — NOT in this spec, even though
    // bash's own usage string lists both. huck has no initial-word spec
    // slot (`CompletionSpecs` has only `by_command`/`default_spec`/
    // `empty_spec`) and no command-runner completion generator
    // (`CompletionSpec` has no field for it). Accepting either would parse
    // successfully and then either panic (no match arm to route it to) or
    // require inventing throwaway storage nothing reads — the #496 Task 6
    // mapfile lesson (silently-wrong beats loudly-rejected only when it's
    // actually implemented). Pre-v359 huck already rejected both
    // (`-C: invalid option`, `-I: invalid option`); this keeps that.
    let spec: &str = if allow_d_e {
        "abcdefgjksuvprDEo:A:G:W:F:X:P:S:"
    } else {
        "abcdefgjksuvo:A:G:W:F:X:P:S:"
    };
    // bash's `complete`/`compgen` do NOT parse `+` at all (#515): measured,
    // `complete +o nospace foo` registers THREE NAMES — `foo`, `nospace` and
    // `+o` — each with an empty compspec. huck used to run a `+` loop here,
    // alternating with the `-` scan; that loop is gone, so one pass suffices.
    // (`compopt` is different: its `+o` really does remove an option.)
    let mut g =
        crate::builtin_opts::Getopt::new(name, crate::builtin_opts::ArgView::Plain(args), spec);
    loop {
        match g.next_opt(shell, err) {
            Ok(Some(o)) => {
                out.saw_option = true;
                match o.ch {
                    'F' => {
                        let v = o.value.expect("F takes a value");
                        // #550: bash validates the function name at
                        // REGISTRATION and refuses the whole command. The rule
                        // is not "identifier" despite the wording — `1abc`,
                        // `a-b`, `a.b`, `a/b`, `a$b` and the empty string are
                        // all accepted. Measured, what it rejects is a name
                        // containing a shell BREAK character (`shellbreak()` in
                        // bash: ` \t\n;|&()<>`), which is also why `-F` can be
                        // printed unquoted by `complete -p` (#527).
                        if v.contains([' ', '\t', '\n', ';', '|', '&', '(', ')', '<', '>']) {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "{name}: `{v}': not a valid identifier"
                            );
                            return Err(2);
                        }
                        out.spec.function = Some(v);
                    }
                    'W' => out.spec.wordlist = Some(o.value.expect("W takes a value")),
                    'G' => out.spec.glob = Some(o.value.expect("G takes a value")),
                    'A' => {
                        let v = o.value.expect("A takes a value");
                        match Action::parse(&v) {
                            Some(action) => out.spec.actions.push(action),
                            None => {
                                crate::sh_error_to!(
                                    shell,
                                    err,
                                    None,
                                    "{name}: {v}: invalid action name"
                                );
                                return Err(2);
                            }
                        }
                    }
                    'P' => out.spec.prefix = Some(o.value.expect("P takes a value")),
                    'S' => out.spec.suffix = Some(o.value.expect("S takes a value")),
                    'X' => out.spec.filter = Some(o.value.expect("X takes a value")),
                    'o' => {
                        let v = o.value.expect("o takes a value");
                        if apply_option(&mut out.spec.options, &v, false).is_err() {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "{name}: {v}: invalid completion option"
                            );
                            return Err(2);
                        }
                    }
                    'D' if allow_d_e => out.is_default = true,
                    'E' if allow_d_e => out.is_empty = true,
                    'p' if allow_d_e => out.print = true,
                    'r' if allow_d_e => out.remove = true,
                    'a' | 'b' | 'c' | 'd' | 'e' | 'f' | 'g' | 'j' | 'k' | 's' | 'u' | 'v' => {
                        let action = match o.ch {
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
                    _ => return Err(g.reject_unhandled(o.ch, shell, err)),
                }
            }
            Ok(None) => break,
            Err(code) => return Err(code),
        }
    }
    out.positional = args[g.rest_index()..].to_vec();
    Ok(out)
}

fn apply_option(opts: &mut CompOptions, name: &str, off: bool) -> Result<(), ()> {
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
        _ => return Err(()),
    }
    Ok(())
}

/// `complete` builtin.
pub fn builtin_complete(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let parsed = match parse_flags(args, true, "complete", shell, err) {
        Ok(p) => p,
        Err(code) => return ExecOutcome::Continue(code),
    };

    // Mode: print
    if parsed.print || is_bare(&parsed) {
        return print_complete(
            &parsed.positional,
            parsed.is_default,
            parsed.is_empty,
            out,
            err,
            shell,
        );
    }
    // Mode: remove
    if parsed.remove {
        return remove_complete(&parsed.positional, &parsed, err, shell);
    }
    // Mode: register
    register_complete(&parsed, err, shell)
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
    err: &mut dyn Write,
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
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "complete: no completion specification for -D"
                );
                status = 1;
            }
        }
    }
    if is_empty {
        match &specs.empty_spec {
            Some(es) => {
                let _ = writeln!(out, "{}", format_spec_for_print(es, None, Some("-E")));
            }
            None => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "complete: no completion specification for -E"
                );
                status = 1;
            }
        }
    }
    if is_default || is_empty {
        return ExecOutcome::Continue(status);
    }

    if names.is_empty() {
        // #527: bash keeps every compspec — command names AND the `-D`/`-E`
        // slots — in ONE hash table and `complete -p` walks it, so the output
        // order is neither sorted nor insertion order, and the `-D` line is
        // NOT pinned to the end: it lands wherever `_DefaultCmD_` hashes.
        for key in specs.keys_in_bash_order() {
            let line = match key {
                CompKey::Command(n) => {
                    format_spec_for_print(&specs.by_command[n], Some(n.as_str()), None)
                }
                CompKey::Default => {
                    format_spec_for_print(specs.default_spec.as_ref().unwrap(), None, Some("-D"))
                }
                CompKey::Empty => {
                    format_spec_for_print(specs.empty_spec.as_ref().unwrap(), None, Some("-E"))
                }
            };
            let _ = writeln!(out, "{line}");
        }
    } else {
        for n in names {
            match specs.by_command.get(n) {
                Some(s) => {
                    let _ = writeln!(out, "{}", format_spec_for_print(s, Some(n.as_str()), None));
                }
                None => {
                    crate::sh_error_to!(
                        shell,
                        err,
                        None,
                        "complete: {n}: no completion specification"
                    );
                    status = 1;
                }
            }
        }
    }
    ExecOutcome::Continue(status)
}

fn remove_complete(
    names: &[String],
    parsed: &ParsedFlags,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut status = 0;
    let specs = Rc::make_mut(&mut shell.completion_specs);
    if parsed.is_default {
        specs.unregister_slot(CompKey::Default);
    }
    if parsed.is_empty {
        specs.unregister_slot(CompKey::Empty);
    }
    if names.is_empty() && !parsed.is_default && !parsed.is_empty {
        specs.clear_commands();
    } else {
        // Collect misses first: `specs` borrows `shell.completion_specs`
        // mutably for the whole loop, so the diagnostic (which needs
        // `shell` itself, for the error prologue) must be emitted after
        // the loop, once that borrow has ended.
        let mut missing: Vec<&String> = Vec::new();
        for n in names {
            if !specs.unregister(n) && !parsed.is_default && !parsed.is_empty {
                missing.push(n);
                status = 1;
            }
        }
        for n in missing {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "complete: {n}: no completion specification"
            );
        }
    }
    ExecOutcome::Continue(status)
}

fn register_complete(parsed: &ParsedFlags, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if (parsed.is_default || parsed.is_empty) && !parsed.positional.is_empty() {
        crate::sh_error_to!(
            shell,
            err,
            None,
            "complete: cannot use -D or -E with command names"
        );
        return ExecOutcome::Continue(2);
    }
    // No "nothing to complete" guard: bash registers an EMPTY compspec quite
    // happily. Measured — `complete foo; complete -p` prints `complete foo`,
    // rc 0. The guard rejected that outright, and also swallowed the names in
    // `complete +z foo` and `complete -- -o foo`, both of which bash registers
    // (#515). Nothing in the tree pinned it.
    let specs = Rc::make_mut(&mut shell.completion_specs);
    if parsed.is_default {
        specs.register_slot(CompKey::Default, parsed.spec.clone());
    }
    if parsed.is_empty {
        specs.register_slot(CompKey::Empty, parsed.spec.clone());
    }
    for n in &parsed.positional {
        specs.register(n, parsed.spec.clone());
    }
    ExecOutcome::Continue(0)
}

/// bash's `compacts[]` table (pcomplete.c): every `-A` action name in the
/// order `complete -p` prints them, paired with its one-letter option where
/// bash has one. Printing walks the table TWICE — the short flags first, then
/// the `-A name` forms — which is why `complete -u -v -A hostname` comes out
/// in that order however the flags were typed.
const COMPACTS: &[(Action, Option<char>)] = &[
    (Action::Alias, Some('a')),
    (Action::Arrayvar, None),
    (Action::Binding, None),
    (Action::Builtin, Some('b')),
    (Action::Command, Some('c')),
    (Action::Directory, Some('d')),
    (Action::Disabled, None),
    (Action::Enabled, None),
    (Action::Export, Some('e')),
    (Action::File, Some('f')),
    (Action::Function, None),
    (Action::Group, Some('g')),
    (Action::Helptopic, None),
    (Action::Hostname, None),
    (Action::Job, Some('j')),
    (Action::Keyword, Some('k')),
    (Action::Running, None),
    (Action::Service, Some('s')),
    (Action::Setopt, None),
    (Action::Shopt, None),
    (Action::Signal, None),
    (Action::Stopped, None),
    (Action::User, Some('u')),
    (Action::Variable, Some('v')),
];

/// bash's `sh_contains_shell_metas` (lib/sh/shquote.c): whether a word has to
/// be quoted to survive re-input. `complete -p` runs the COMMAND NAME through
/// this and single-quotes only when it says yes — which is why bash prints
/// `complete a~b` but `complete 'a^b'`. Option VALUES are unconditionally
/// quoted instead, so this is not used for them.
fn contains_shell_metas(s: &str) -> bool {
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b' ' | b'\t' | b'\n' => return true,
            b'\'' | b'"' | b'\\' => return true,
            b'|' | b'&' | b';' | b'(' | b')' | b'<' | b'>' => return true,
            b'!' | b'{' | b'}' => return true,
            b'*' | b'[' | b'?' | b']' | b'^' => return true,
            b'$' | b'`' => return true,
            // Tilde expansion only triggers at the start of the word or right
            // after `=` / `:`, and `#` only starts a comment at the start.
            b'~' if i == 0 || b[i - 1] == b'=' || b[i - 1] == b':' => return true,
            b'#' if i == 0 => return true,
            _ => {}
        }
    }
    false
}

/// A word as `complete -p` writes it: bare when it needs no quoting,
/// single-quoted (bash's `sh_single_quote`) when it does.
///
/// `quote_empty` is the one difference between the two words that get this
/// treatment: an empty command NAME prints as `''` (so `complete ""` round
/// trips) while an empty `-F` value prints BARE, leaving `complete -F  e`
/// with two spaces — measured, and bash's own output does not re-input there.
/// Every OTHER option value (`-W`, `-P`, `-S`, `-X`, `-G`) is quoted
/// unconditionally, so it does not come through here.
fn print_quote_word(word: &str, quote_empty: bool) -> String {
    if (word.is_empty() && quote_empty) || contains_shell_metas(word) {
        format!("'{}'", crate::builtins::escape_alias_value(word))
    } else {
        word.to_string()
    }
}

/// Renders a spec for `complete -p` in bash's re-input form: options, then
/// actions, then the generators, then `-D`/`-E`, then the name.
fn format_spec_for_print(spec: &CompletionSpec, name: Option<&str>, mode: Option<&str>) -> String {
    let mut parts: Vec<String> = vec!["complete".to_string()];
    // `-o` options come first, in bash's `compopts[]` table order (which is
    // alphabetical) — NOT the order they were given on the command line.
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
    for (on, opt) in [
        (bashdefault, "bashdefault"),
        (default, "default"),
        (dirnames, "dirnames"),
        (filenames, "filenames"),
        (noquote, "noquote"),
        (nosort, "nosort"),
        (nospace, "nospace"),
        (plusdirs, "plusdirs"),
    ] {
        if on {
            parts.push(format!("-o {opt}"));
        }
    }
    // Actions: short flags first, then the long `-A` forms, each in table
    // order. bash stores actions as a BITMASK, so a repeated `-u -u` collapses
    // and the order the user typed is not recoverable — matching that means
    // walking the table rather than `spec.actions`.
    for (act, ch) in COMPACTS {
        if let Some(c) = ch
            && spec.actions.contains(act)
        {
            parts.push(format!("-{c}"));
        }
    }
    for (act, ch) in COMPACTS {
        if ch.is_none() && spec.actions.contains(act) {
            parts.push(format!("-A {}", act.as_str()));
        }
    }
    if let Some(g) = &spec.glob {
        parts.push(format!("-G '{}'", crate::builtins::escape_alias_value(g)));
    }
    if let Some(w) = &spec.wordlist {
        parts.push(format!("-W '{}'", crate::builtins::escape_alias_value(w)));
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
    // `-F` is quoted only when the name needs it — the same test as the command
    // name, but an EMPTY value stays bare (#550).
    if let Some(f) = &spec.function {
        parts.push(format!("-F {}", print_quote_word(f, false)));
    }
    // `-D` / `-E` sit at the END of the line, where the command name would be.
    if let Some(m) = mode {
        parts.push(m.to_string());
    }
    if let Some(n) = name {
        parts.push(print_quote_word(n, true));
    }
    parts.join(" ")
}

/// `compgen` builtin.
pub fn builtin_compgen(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let parsed = match parse_flags(args, false, "compgen", shell, err) {
        Ok(p) => p,
        Err(code) => return ExecOutcome::Continue(code),
    };

    // #528: `compgen` with NO options generates nothing and exits 0, whatever
    // words follow. bash's `build_actions` returns EXECUTION_FAILURE when its
    // getopt loop consumed no option, and `compgen_builtin` maps exactly that
    // failure back to EXECUTION_SUCCESS — so `compgen zzz`, `compgen +z` (`+z`
    // is a plain word, #515), `compgen -- -W abc` and bare `compgen` are all
    // silent successes, while `compgen -o nospace zzz` (an option, no matches)
    // is the ordinary status 1. A `--` alone is not an option.
    if !parsed.saw_option {
        return ExecOutcome::Continue(0);
    }

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
pub fn builtin_compopt(
    args: &[String],
    _out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut option_set: Vec<(String, bool)> = Vec::new();
    let mut is_default = false;
    let mut is_empty = false;

    // `I` (bash: "change options for completion on the initial word") is a
    // real, bash-implemented option NOT in this spec: huck has no
    // initial-word compspec slot to route it to (same reasoning as
    // `complete`/`compgen`'s `parse_flags` above). Pre-v359 huck already
    // rejected it (`-I: invalid option`); this keeps that.
    //
    // The `-`-side is the shared scanner (#496); `+o` is NOT — `Getopt`
    // doesn't understand `+` (neither does bash's own `internal_getopt`
    // here), so this alternates scanner-run / one-`+`-arg exactly like
    // `declare` (Task 4) and `complete`/`compgen` (`parse_flags` above).
    let mut idx = 0;
    loop {
        let pre_idx = idx;
        let mut g = crate::builtin_opts::Getopt::new(
            "compopt",
            crate::builtin_opts::ArgView::Plain(&args[idx..]),
            "DEo:",
        );
        loop {
            match g.next_opt(shell, err) {
                Ok(Some(o)) => match o.ch {
                    'o' => {
                        let v = o.value.expect("o takes a value");
                        if !["default", "nospace", "filenames", "bashdefault", "dirnames"]
                            .contains(&v.as_str())
                        {
                            crate::sh_error_to!(
                                shell,
                                err,
                                None,
                                "compopt: {v}: invalid completion option"
                            );
                            return ExecOutcome::Continue(2);
                        }
                        option_set.push((v, false));
                    }
                    'D' => is_default = true,
                    'E' => is_empty = true,
                    _ => return ExecOutcome::Continue(g.reject_unhandled(o.ch, shell, err)),
                },
                Ok(None) => break,
                Err(code) => return ExecOutcome::Continue(code),
            }
        }
        idx += g.rest_index();

        if idx > pre_idx && args.get(idx - 1).map(String::as_str) == Some("--") {
            break;
        }

        let Some(arg) = args.get(idx) else { break };
        if !(arg.starts_with('+') && arg.len() > 1) {
            break;
        }
        // ---- verbatim pre-v359 `+`-run handling ----
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
                    } else if idx + 1 < args.len() {
                        idx += 1;
                        ci = chars.len();
                        args[idx].clone()
                    } else {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "compopt: -o: option requires an argument"
                        );
                        return ExecOutcome::Continue(2);
                    };
                    if !["default", "nospace", "filenames", "bashdefault", "dirnames"]
                        .contains(&arg_value.as_str())
                    {
                        crate::sh_error_to!(
                            shell,
                            err,
                            None,
                            "compopt: {arg_value}: invalid completion option"
                        );
                        return ExecOutcome::Continue(2);
                    }
                    option_set.push((arg_value, true));
                }
                'D' => is_default = true,
                'E' => is_empty = true,
                other => {
                    // #521: this reported `-{other}` with no usage line while
                    // `declare`'s sibling loop reported `+{other}` with one.
                    // Both now go through the shared emit.
                    crate::builtin_opts::emit_invalid_plus_option(
                        "compopt",
                        crate::builtin_opts::opt_first_byte(other),
                        shell,
                        err,
                    );
                    return ExecOutcome::Continue(2);
                }
            }
            ci += 1;
        }
        idx += 1;
    }
    let names: Vec<String> = args[idx..].to_vec();

    if is_default || is_empty {
        crate::sh_error_to!(shell, err, None, "compopt: -D/-E not yet supported");
        return ExecOutcome::Continue(2);
    }

    if names.is_empty() {
        // In-function mutation. The dispatch path stashes the live spec
        // in shell.current_completion_spec before invoking -F; we take
        // it out, mutate, and put it back so dispatch's later .take()
        // observes the change.
        let Some(mut live) = shell.current_completion_spec.take() else {
            crate::sh_error_to!(
                shell,
                err,
                None,
                "compopt: not currently executing completion function"
            );
            return ExecOutcome::Continue(1);
        };
        apply_compopt_options(&mut live.options, &option_set);
        shell.current_completion_spec = Some(live);
        return ExecOutcome::Continue(0);
    }

    // Named: mutate registry.
    let mut status = 0;
    for n in &names {
        match Rc::make_mut(&mut shell.completion_specs)
            .by_command
            .get_mut(n)
        {
            Some(spec) => apply_compopt_options(&mut spec.options, &option_set),
            None => {
                crate::sh_error_to!(
                    shell,
                    err,
                    None,
                    "compopt: {n}: no completion specification"
                );
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
        let mut err = Vec::<u8>::new();
        let outcome = builtin_complete(&argv, &mut out, &mut err, shell);
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
        let mut err = Vec::<u8>::new();
        let outcome = builtin_compgen(&argv, &mut out, &mut err, shell);
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
        let mut err = Vec::<u8>::new();
        let outcome = builtin_compopt(&argv, &mut out, &mut err, shell);
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
            &[
                "-o", "nosort", "-o", "noquote", "-o", "plusdirs", "-W", "x", "--", "foo",
            ],
            &mut sh,
        );
        assert_eq!(code, 0);
        let opts = sh.completion_specs.by_command["foo"].options;
        assert!(opts.nosort && opts.noquote && opts.plusdirs);
    }

    #[test]
    fn complete_plus_o_nosort_is_a_name_not_an_option() {
        // Was `complete_plus_o_nosort_clears_it` — same wrong premise as
        // `complete_does_not_parse_plus_o_at_all` above (#515). `compopt +o`
        // IS real and still clears; `complete +o` is not.
        let mut sh = Shell::new();
        let (_, c1) = run_complete(&["-o", "nosort", "-W", "x", "--", "foo"], &mut sh);
        assert_eq!(c1, 0);
        let (_, c2) = run_complete(&["+o", "nosort", "--", "foo"], &mut sh);
        assert_eq!(c2, 0);
        assert!(sh.completion_specs.by_command.contains_key("+o"));
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
    fn complete_bare_name_registers_an_empty_compspec() {
        // Was `complete_nothing_to_complete_errors`, asserting `code == 1`.
        // That encoded a guard huck invented: bash registers an EMPTY compspec
        // quite happily. Measured on bash 5.2.21 —
        //   $ complete foo; complete -p
        //   complete foo            (rc 0)
        // The guard also swallowed the names in `complete +z foo` and
        // `complete -- -o foo`, both of which bash registers (#515).
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["foo"], &mut sh);
        assert_eq!(code, 0, "bash registers an empty compspec, rc 0");
        assert!(sh.completion_specs.by_command.contains_key("foo"));
    }

    #[test]
    fn compgen_W_filters_by_prefix_arg() {
        let mut sh = Shell::new();
        let (out, code) = run_compgen(&["-W", "alpha alpine beta", "al"], &mut sh);
        assert_eq!(code, 0);
        assert_eq!(out, "alpha\nalpine\n");
    }

    #[test]
    fn compgen_output_has_no_trailing_space() {
        // The tab-dispatch trailing space (Tasks 1-2, #42) must NOT reach
        // compgen (bash's compgen lists matches without a trailing space).
        let mut sh = Shell::new();
        let (out, code) = run_compgen(&["-W", "foobar", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert_eq!(
            out, "foobar\n",
            "compgen output must not carry the tab-completion trailing space"
        );
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

        let mut reparse_err = Vec::<u8>::new();
        let parsed = super::parse_flags(&tokens[1..], true, "complete", &mut sh, &mut reparse_err)
            .expect("re-parse");
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
    fn complete_does_not_parse_plus_o_at_all() {
        // Was `complete_plus_o_clears_option`, asserting `complete +o nospace`
        // CLEARS an option. bash does not parse `+` in `complete` at all —
        // measured on bash 5.2.21:
        //   $ complete -W x -o nospace -- foo; complete -p foo
        //   complete -o nospace -W 'x' foo
        //   $ complete +o nospace -- foo; complete -p
        //   complete foo / complete nospace / complete +o / complete --
        // i.e. all four tokens become NAMES and foo's spec is REPLACED by an
        // empty one. The old test passed for the wrong reason after #515: the
        // replacement spec has `nospace == false`, satisfying the assertion
        // without anything having been "cleared".
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-W", "x", "-o", "nospace", "--", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert!(sh.completion_specs.by_command["foo"].options.nospace);

        let (_, code) = run_complete(&["+o", "nospace", "--", "foo"], &mut sh);
        assert_eq!(code, 0);
        for name in ["+o", "nospace", "--", "foo"] {
            assert!(
                sh.completion_specs.by_command.contains_key(name),
                "{name} should have been registered as a NAME"
            );
        }
        assert!(
            !sh.completion_specs.by_command["foo"].options.nospace,
            "foo's spec is replaced by an empty one, not merely cleared"
        );
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
        assert!(
            !out.contains(" -- foo"),
            "should not print foo's spec: {out:?}"
        );
    }

    #[test]
    fn compopt_D_rejected_with_exit_2() {
        let mut sh = Shell::new();
        let (_, code) = run_compopt(&["-D", "-o", "nospace"], &mut sh);
        assert_eq!(
            code, 2,
            "compopt -D is a parse-time rejection, should be exit 2"
        );
    }

    #[test]
    fn compopt_E_rejected_with_exit_2() {
        let mut sh = Shell::new();
        let (_, code) = run_compopt(&["-E", "-o", "nospace"], &mut sh);
        assert_eq!(
            code, 2,
            "compopt -E is a parse-time rejection, should be exit 2"
        );
    }

    #[test]
    fn compgen_F_does_not_leak_current_completion_spec() {
        let mut sh = Shell::new();
        // Define a function and run compgen -F. After it returns,
        // shell.current_completion_spec MUST be None — otherwise the
        // next tab dispatch on an unrelated spec gets the wrong options.
        let _ = crate::shell::process_line("_myf() { COMPREPLY=(a b); }", &mut sh, false);
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
        assert_eq!(
            code, 1,
            "-- should end flags so 'foo' is a name; no spec → exit 1"
        );
    }

    #[test]
    fn complete_short_flag_actions_map_to_actions() {
        use crate::completion_spec::Action;
        let cases = [
            ("-a", Action::Alias),
            ("-b", Action::Builtin),
            ("-c", Action::Command),
            ("-d", Action::Directory),
            ("-e", Action::Export),
            ("-f", Action::File),
            ("-g", Action::Group),
            ("-j", Action::Job),
            ("-k", Action::Keyword),
            ("-s", Action::Service),
            ("-u", Action::User),
            ("-v", Action::Variable),
        ];
        for (flag, want) in cases {
            let mut sh = Shell::new();
            let (_, code) = run_complete(&[flag, "--", "foo"], &mut sh);
            assert_eq!(code, 0, "flag {flag} should be accepted");
            assert_eq!(
                sh.completion_specs.by_command["foo"].actions,
                vec![want],
                "flag {flag} → wrong action"
            );
        }
    }

    #[test]
    fn complete_clustered_short_flags_accumulate() {
        let mut sh = Shell::new();
        let (_, code) = run_complete(&["-ev", "--", "foo"], &mut sh);
        assert_eq!(code, 0);
        assert_eq!(
            sh.completion_specs.by_command["foo"].actions,
            vec![
                crate::completion_spec::Action::Export,
                crate::completion_spec::Action::Variable
            ]
        );
    }
}
