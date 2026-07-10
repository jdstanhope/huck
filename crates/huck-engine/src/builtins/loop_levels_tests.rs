use super::*;
use crate::shell_state::Shell;

// ----- break: valid levels (terminal $? = 0) -----

#[test]
fn break_no_args_emits_level_1_status_0() {
    let mut sh = Shell::new();
    sh.loop_depth = 1;
    let outcome = builtin_break(&[], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopBreak(1, 0));
}

#[test]
fn break_with_arg_n_emits_level_n_when_in_loop() {
    let mut sh = Shell::new();
    sh.loop_depth = 3;
    let outcome = builtin_break(&["2".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopBreak(2, 0));
}

#[test]
fn break_caps_to_loop_depth() {
    let mut sh = Shell::new();
    sh.loop_depth = 2;
    let outcome = builtin_break(&["999".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopBreak(2, 0));
}

// ----- break: outside a loop → exit status 0, no break -----

#[test]
fn break_outside_loop_errors_with_status_0() {
    let sh = Shell::new();
    // sh.loop_depth = 0 by default.
    // Bash 5.2: break/continue outside a loop prints the diagnostic to
    // stderr but returns $? = 0 and does NOT break anything. Arg
    // validation is skipped entirely.
    assert_eq!(
        builtin_break(&[], &mut std::io::stderr(), &sh),
        ExecOutcome::Continue(0)
    );
    assert_eq!(
        builtin_break(&["abc".to_string()], &mut std::io::stderr(), &sh),
        ExecOutcome::Continue(0)
    );
    assert_eq!(
        builtin_break(&["0".to_string()], &mut std::io::stderr(), &sh),
        ExecOutcome::Continue(0)
    );
    assert_eq!(
        builtin_break(
            &["1".to_string(), "2".to_string(), "3".to_string()],
            &mut std::io::stderr(),
            &sh
        ),
        ExecOutcome::Continue(0)
    );
}

// ----- break: malformed N<=0 → break ALL loops, terminal $? = 1 -----

#[test]
fn break_zero_breaks_all_loops_status_1() {
    let mut sh = Shell::new();
    sh.loop_depth = 2;
    let outcome = builtin_break(&["0".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopBreak(2, 1));
}

#[test]
fn break_negative_breaks_all_loops_status_1() {
    let mut sh = Shell::new();
    sh.loop_depth = 1;
    let outcome = builtin_break(&["-1".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopBreak(1, 1));
}

// ----- break: too many args → break ALL loops, terminal $? = 1 -----

#[test]
fn break_too_many_args_breaks_all_loops_status_1() {
    let mut sh = Shell::new();
    sh.loop_depth = 2;
    let outcome = builtin_break(
        &["1".to_string(), "2".to_string()],
        &mut std::io::stderr(),
        &sh,
    );
    assert_eq!(outcome, ExecOutcome::LoopBreak(2, 1));
}

// ----- break: non-numeric → abort script with exit 128 -----

#[test]
fn break_non_numeric_exits_with_status_128() {
    let mut sh = Shell::new();
    sh.loop_depth = 1;
    let outcome = builtin_break(&["abc".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::Exit(128));
}

// ----- continue: valid levels (LoopContinue) -----

#[test]
fn continue_no_args_emits_level_1() {
    let mut sh = Shell::new();
    sh.loop_depth = 1;
    let outcome = builtin_continue(&[], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopContinue(1));
}

#[test]
fn continue_caps_to_loop_depth() {
    let mut sh = Shell::new();
    sh.loop_depth = 1;
    let outcome = builtin_continue(&["5".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopContinue(1));
}

// ----- continue: outside a loop → exit status 0, no continue -----

#[test]
fn continue_outside_loop_errors_with_status_0() {
    let sh = Shell::new();
    assert_eq!(
        builtin_continue(&[], &mut std::io::stderr(), &sh),
        ExecOutcome::Continue(0)
    );
    assert_eq!(
        builtin_continue(&["abc".to_string()], &mut std::io::stderr(), &sh),
        ExecOutcome::Continue(0)
    );
    assert_eq!(
        builtin_continue(&["0".to_string()], &mut std::io::stderr(), &sh),
        ExecOutcome::Continue(0)
    );
}

// ----- continue: malformed N<=0 / too-many → break ALL loops, $? = 1 -----

#[test]
fn continue_zero_breaks_all_loops_status_1() {
    let mut sh = Shell::new();
    sh.loop_depth = 2;
    let outcome = builtin_continue(&["0".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopBreak(2, 1));
}

#[test]
fn continue_negative_breaks_all_loops_status_1() {
    let mut sh = Shell::new();
    sh.loop_depth = 3;
    let outcome = builtin_continue(&["-5".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::LoopBreak(3, 1));
}

#[test]
fn continue_too_many_args_breaks_all_loops_status_1() {
    let mut sh = Shell::new();
    sh.loop_depth = 2;
    let outcome = builtin_continue(
        &["1".to_string(), "2".to_string()],
        &mut std::io::stderr(),
        &sh,
    );
    assert_eq!(outcome, ExecOutcome::LoopBreak(2, 1));
}

// ----- continue: non-numeric → abort script with exit 128 -----

#[test]
fn continue_non_numeric_exits_with_status_128() {
    let mut sh = Shell::new();
    sh.loop_depth = 1;
    let outcome = builtin_continue(&["abc".to_string()], &mut std::io::stderr(), &sh);
    assert_eq!(outcome, ExecOutcome::Exit(128));
}
