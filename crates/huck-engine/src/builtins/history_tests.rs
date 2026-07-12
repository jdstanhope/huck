use super::*;
use crate::shell_state::Shell;

#[test]
fn history_lists_numbered_entries() {
    let mut shell = Shell::new();
    Rc::make_mut(&mut shell.history).add("first cmd".to_string());
    Rc::make_mut(&mut shell.history).add("second cmd".to_string());
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("history", &[], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("first cmd"), "output: {text}");
    assert!(text.contains("second cmd"), "output: {text}");
    assert!(text.contains("1"), "output should have numbers: {text}");
}

#[test]
fn history_dash_c_clears() {
    let mut shell = Shell::new();
    Rc::make_mut(&mut shell.history).add("doomed".to_string());
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &["-c".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.history.last(), None);
}

#[test]
fn history_invalid_option_errors() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &["--bogus".to_string()],
        &mut out,
        &mut err,
        &mut shell,
    );
    // Matches bash: an unrecognized option is a usage error (rc 2), with a
    // usage line on stderr, and sets the special-builtin usage-error status.
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
    let err_text = String::from_utf8(err).unwrap();
    assert!(
        err_text.contains("--bogus: invalid option"),
        "err: {err_text}"
    );
    assert!(err_text.contains("history: usage:"), "err: {err_text}");
}

#[test]
fn history_n_prints_last_n() {
    let mut shell = Shell::new();
    for c in ["a", "b", "c"] {
        Rc::make_mut(&mut shell.history).add(c.to_string());
    }
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &["2".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(String::from_utf8(out).unwrap(), "    2  b\n    3  c\n");
}

#[test]
fn history_n_zero_prints_nothing() {
    let mut shell = Shell::new();
    Rc::make_mut(&mut shell.history).add("a".to_string());
    let mut out: Vec<u8> = Vec::new();
    run_builtin(
        "history",
        &["0".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert_eq!(out, b"");
}

#[test]
fn history_d_deletes_and_out_of_range_errors() {
    let mut shell = Shell::new();
    for c in ["a", "b", "c"] {
        Rc::make_mut(&mut shell.history).add(c.to_string());
    }
    let mut err: Vec<u8> = Vec::new();
    let ok = run_builtin(
        "history",
        &["-d".to_string(), "2".to_string()],
        &mut Vec::new(),
        &mut err,
        &mut shell,
    );
    assert!(matches!(ok, ExecOutcome::Continue(0)));
    assert_eq!(
        shell.history.entries().collect::<Vec<_>>(),
        vec![(1, "a"), (2, "c")]
    );
    let bad = run_builtin(
        "history",
        &["-d".to_string(), "9".to_string()],
        &mut Vec::new(),
        &mut err,
        &mut shell,
    );
    assert!(matches!(bad, ExecOutcome::Continue(1)));
    assert!(
        String::from_utf8(err)
            .unwrap()
            .contains("history position out of range")
    );
}

#[test]
fn history_d_empty_operand_does_not_panic() {
    // Finding 1 crash guard: `history -d ''` must not panic on the empty
    // operand byte-slice; it falls through to the single-offset path and
    // fails with the standard out-of-range error, rc 1.
    let mut shell = Shell::new();
    Rc::make_mut(&mut shell.history).add("a".to_string());
    let mut err: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &["-d".to_string(), String::new()],
        &mut Vec::new(),
        &mut err,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(
        String::from_utf8(err)
            .unwrap()
            .contains("history position out of range")
    );
}

#[test]
fn history_double_dash_lists_all() {
    // Finding 2: `--` is end-of-options, not an action; with nothing after
    // it, the whole history is listed (rc 0).
    let mut shell = Shell::new();
    Rc::make_mut(&mut shell.history).add("first cmd".to_string());
    Rc::make_mut(&mut shell.history).add("second cmd".to_string());
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &["--".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("first cmd"), "output: {text}");
    assert!(text.contains("second cmd"), "output: {text}");
}

#[test]
fn history_dash_c_still_lists_nothing() {
    // Finding 2 regression guard: an actual action (-c) must still suppress
    // the "list all" fallback.
    let mut shell = Shell::new();
    Rc::make_mut(&mut shell.history).add("doomed".to_string());
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &["-c".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(out, b"");
}

#[test]
fn history_nonnumeric_count_arg_errors() {
    // Finding 3: a non-numeric non-option operand is "numeric argument
    // required", rc 1 (NOT "invalid option" / rc 2).
    let mut shell = Shell::new();
    let mut err: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &["abc".to_string()],
        &mut Vec::new(),
        &mut err,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    let err_text = String::from_utf8(err).unwrap();
    assert!(
        err_text.contains("abc: numeric argument required"),
        "err: {err_text}"
    );
}

#[test]
fn history_too_many_arguments_errors() {
    // Finding 4: more than one trailing non-option operand is an error, not
    // a silent listing.
    let mut shell = Shell::new();
    for c in ["a", "b", "c"] {
        Rc::make_mut(&mut shell.history).add(c.to_string());
    }
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &["2".to_string(), "3".to_string()],
        &mut out,
        &mut err,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert_eq!(out, b"");
    assert!(
        String::from_utf8(err)
            .unwrap()
            .contains("history: too many arguments")
    );
}

#[test]
fn history_extra_operands_after_action_are_ignored() {
    // Bash 5.2 ground truth: once an option has actually performed an
    // action (-c/-d/-w/-r/-a), any leftover trailing operands are silently
    // discarded -- neither a "too many arguments" error nor a listing.
    // E.g. `history -d 2 3 4` deletes offset 2 and ignores "3" and "4"
    // entirely (no numeric-argument check, no too-many-arguments check).
    let mut shell = Shell::new();
    for c in ["a", "b", "c"] {
        Rc::make_mut(&mut shell.history).add(c.to_string());
    }
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "history",
        &[
            "-d".to_string(),
            "2".to_string(),
            "3".to_string(),
            "abc".to_string(),
        ],
        &mut out,
        &mut err,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(out, b"");
    assert_eq!(err, b"");
    assert_eq!(
        shell.history.entries().collect::<Vec<_>>(),
        vec![(1, "a"), (2, "c")]
    );
}

#[test]
fn history_d_negative_and_range() {
    let mut shell = Shell::new();
    for c in ["a", "b", "c", "d", "e"] {
        Rc::make_mut(&mut shell.history).add(c.to_string());
    }
    run_builtin(
        "history",
        &["-d".to_string(), "-1".to_string()],
        &mut Vec::new(),
        &mut std::io::stderr(),
        &mut shell,
    );
    assert_eq!(shell.history.last(), Some("d")); // e removed
    run_builtin(
        "history",
        &["-d".to_string(), "2-3".to_string()],
        &mut Vec::new(),
        &mut std::io::stderr(),
        &mut shell,
    );
    assert_eq!(
        shell.history.entries().collect::<Vec<_>>(),
        vec![(1, "a"), (2, "d")]
    );
}
