use super::*;
use crate::shell_state::Shell;

fn run(args: &[&str]) -> (ExecOutcome, String) {
    let mut shell = Shell::new();
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "help",
        &args_owned,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    (outcome, String::from_utf8(buf).unwrap())
}

#[test]
fn help_no_args_lists_all() {
    let (oc, out) = run(&[]);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    // Sample a few we know exist.
    assert!(out.lines().any(|l| l.starts_with("cd:")));
    assert!(out.lines().any(|l| l.starts_with("echo:")));
    assert!(out.lines().any(|l| l.starts_with("eval:")));
}

#[test]
fn help_named_builtin_default_form() {
    let (oc, out) = run(&["cd"]);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(out.lines().any(|l| l.starts_with("cd:")));
    // At least one indented continuation line.
    assert!(out.lines().any(|l| l.starts_with("    ")));
}

#[test]
fn help_synopsis_only() {
    let (oc, out) = run(&["-s", "echo"]);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    // Exactly one line starting with "echo:"; no indentation.
    let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].starts_with("echo:"));
}

#[test]
fn help_description_only() {
    let (oc, out) = run(&["-d", "echo"]);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    // No line starts with "echo:".
    assert!(out.lines().all(|l| !l.starts_with("echo:")));
    // Has actual description text.
    assert!(!out.trim().is_empty());
}

#[test]
fn help_man_format() {
    let (oc, out) = run(&["-m", "echo"]);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(out.lines().any(|l| l == "NAME"));
    assert!(out.lines().any(|l| l == "SYNOPSIS"));
    assert!(out.lines().any(|l| l == "DESCRIPTION"));
}

#[test]
fn help_invalid_option() {
    let (oc, _) = run(&["-X"]);
    assert!(matches!(oc, ExecOutcome::Continue(2)));
}

#[test]
fn help_not_found() {
    let (oc, _) = run(&["__no_such_builtin__"]);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn help_multi_name_partial_miss() {
    let (oc, out) = run(&["cd", "__no_such_builtin__"]);
    // Overall exit 1 because of the miss; cd's content still in stdout.
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    assert!(out.lines().any(|l| l.starts_with("cd:")));
}

#[test]
fn help_keyword_lookup_works() {
    // Shell keywords (if/for/while/etc.) have their own HelpEntry
    // alongside builtins, so `help if` resolves rather than
    // erroring with "no help topics match".
    for kw in [
        "if", "for", "while", "case", "function", "[[", "{", "select",
    ] {
        let (oc, out) = run(&[kw]);
        assert!(
            matches!(oc, ExecOutcome::Continue(0)),
            "expected exit 0 for `help {kw}`",
        );
        assert!(
            out.lines().any(|l| l.starts_with(&format!("{kw}:"))),
            "expected `{kw}:` line in stdout for `help {kw}`; got: {out:?}",
        );
    }
}
