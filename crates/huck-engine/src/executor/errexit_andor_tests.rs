use super::*;
use crate::shell_state::Shell;

/// Run a fragment under `set -e` and report whether errexit fired
/// (i.e. the sequence returned `ExecOutcome::Exit`). Mirrors the
/// non-interactive execute path.
fn errexit_fired(src: &str) -> bool {
    let mut s = Shell::new();
    s.shell_options.errexit = true;
    let mut buf = String::from(src);
    if !buf.ends_with('\n') {
        buf.push('\n');
    }
    let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        &buf,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .expect("parse ok")
    .expect("non-empty parse");
    matches!(execute(&seq, &mut s, &buf), ExecOutcome::Exit(_))
}

#[test]
fn nonlast_andor_failure_is_exempt() {
    // A failing command that is NOT the syntactically last in an and-or
    // list does not trigger errexit — whether the next connector is `&&`
    // or `||`.
    assert!(
        !errexit_fired("false && echo x"),
        "false && echo x must not exit"
    );
    assert!(
        !errexit_fired("false && true"),
        "false && true must not exit"
    );
    assert!(
        !errexit_fired("false && false"),
        "false && false must not exit"
    );
    assert!(
        !errexit_fired("true && false && echo x"),
        "middle-fail must not exit"
    );
    assert!(
        !errexit_fired("false && echo x && echo y"),
        "non-last && must not exit"
    );
    assert!(
        !errexit_fired("false && echo a || echo b"),
        "false && a || b must not exit"
    );
}

#[test]
fn last_andor_failure_triggers_errexit() {
    // The syntactically last command failing DOES trigger errexit.
    assert!(
        errexit_fired("echo a && false"),
        "echo a && false must exit"
    );
    assert!(errexit_fired("true && false"), "true && false must exit");
    assert!(errexit_fired("false || false"), "false || false must exit");
    assert!(
        errexit_fired("echo a && echo b && false"),
        "trailing false must exit"
    );
    assert!(errexit_fired("false"), "bare false must exit");
}

#[test]
fn or_short_circuit_unchanged() {
    // `||` short-circuit behavior is unchanged: a leading false handled by
    // a following `||` clause is exempt (regression guard for the fix,
    // which generalized `!next_is_or` to `is_last`).
    assert!(
        !errexit_fired("false || echo x"),
        "false || echo x must not exit"
    );
    assert!(
        !errexit_fired("true || false"),
        "true || false must not exit"
    );
}

/// v353 Task 2 characterisation: pins the epilogue's ORDER before it is
/// extracted into `finish_command`. The ERR trap must run before errexit ends
/// the sequence, and the action must stay transparent to `$?` (#437) so
/// errexit still reports the FAILING command's status, not the action's.
#[test]
fn err_trap_runs_before_errexit_and_keeps_the_failing_status() {
    let mut s = Shell::new();
    s.shell_options.errexit = true;
    // The action succeeds (status 0). If its status leaked, errexit would
    // carry 0 instead of `false`'s 1.
    let _ = crate::shell::process_line("trap 'true' ERR", &mut s, false);
    let out = crate::shell::process_line("false", &mut s, false);
    assert!(
        matches!(out, ExecOutcome::Exit(1)),
        "expected Exit(1) from errexit after the ERR trap ran, got {out:?}"
    );
}

/// The ERR trap fires only for the syntactically LAST command of an and-or
/// list, and errexit is NOT gated on the trap being armed (#444).
#[test]
fn errexit_fires_without_an_err_trap_installed() {
    assert!(
        errexit_fired("false"),
        "errexit must fire with no ERR trap installed"
    );
    assert!(
        !errexit_fired("false || true"),
        "a non-last failure stays exempt"
    );
}
