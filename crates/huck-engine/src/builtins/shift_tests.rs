use super::*;
use crate::shell_state::Shell;

#[test]
fn shift_no_args_removes_first() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("shift", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.positional_args, vec!["b", "c"]);
}

#[test]
fn shift_n_removes_n() {
    let mut shell = Shell::new();
    shell.positional_args = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "shift",
        &["2".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.positional_args, vec!["c", "d"]);
}

#[test]
fn shift_default_when_no_args_equals_one() {
    let mut shell_a = Shell::new();
    shell_a.positional_args = vec!["x".to_string(), "y".to_string()];
    let mut shell_b = Shell::new();
    shell_b.positional_args = vec!["x".to_string(), "y".to_string()];

    let mut buf: Vec<u8> = Vec::new();
    let _ = run_builtin("shift", &[], &mut buf, &mut std::io::stderr(), &mut shell_a);
    let _ = run_builtin(
        "shift",
        &["1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell_b,
    );

    assert_eq!(shell_a.positional_args, shell_b.positional_args);
    assert_eq!(shell_a.positional_args, vec!["y"]);
}

#[test]
fn shift_too_large_fails_status_1_silently() {
    // bash: an over-range positive count fails (rc 1) SILENTLY — no message.
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string()];
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "shift",
        &["5".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    // Positional unchanged after the failed shift.
    assert_eq!(shell.positional_args, vec!["a"]);
}

#[test]
fn shift_negative_count_errors_status_1() {
    // bash: a negative count is "shift count out of range" (rc 1), distinct
    // from the non-numeric "numeric argument required".
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string()];
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "shift",
        &["-1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert_eq!(shell.positional_args, vec!["a", "b"]);
}

#[test]
fn shift_zero_is_noop() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string()];
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "shift",
        &["0".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.positional_args, vec!["a", "b"]);
}

#[test]
fn shift_non_numeric_errors_status_1() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string()];
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "shift",
        &["abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert_eq!(shell.positional_args, vec!["a"]);
}

#[test]
fn shift_negative_errors_status_1() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string()];
    let mut buf: Vec<u8> = Vec::new();
    // `-1` fails parse::<usize>() because usize can't be negative.
    let outcome = run_builtin(
        "shift",
        &["-1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert_eq!(shell.positional_args, vec!["a", "b"]);
}
