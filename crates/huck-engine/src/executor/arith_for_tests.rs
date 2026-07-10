use crate::builtins::ExecOutcome;
use crate::shell_state::Shell;

#[test]
fn arith_command_nonzero_exits_0() {
    let mut sh = Shell::new();
    let outcome = crate::shell::process_line("((1+2))", &mut sh, false);
    assert!(
        matches!(outcome, ExecOutcome::Continue(0)),
        "got {outcome:?}"
    );
}

#[test]
fn arith_command_zero_exits_1() {
    let mut sh = Shell::new();
    let outcome = crate::shell::process_line("((0))", &mut sh, false);
    assert!(
        matches!(outcome, ExecOutcome::Continue(1)),
        "got {outcome:?}"
    );
}

#[test]
fn arith_command_division_by_zero_exits_1() {
    let mut sh = Shell::new();
    let outcome = crate::shell::process_line("((1/0))", &mut sh, false);
    assert!(
        matches!(outcome, ExecOutcome::Continue(1)),
        "got {outcome:?}"
    );
}

#[test]
fn arith_for_counter_loop_sets_var() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line("for ((i=0;i<3;i++)) do :; done", &mut sh, false);
    // After the loop, i should be 3 (the value at which cond failed).
    assert_eq!(sh.lookup_var("i").as_deref(), Some("3"));
}

#[test]
fn arith_for_break_stops_at_value() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "for ((i=0;i<10;i++)) do if [ $i -eq 5 ]; then break; fi; done",
        &mut sh,
        false,
    );
    // i was 5 when break fired; step does NOT run after break.
    assert_eq!(sh.lookup_var("i").as_deref(), Some("5"));
}

#[test]
fn arith_for_continue_evaluates_step() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line("for ((i=0;i<5;i++)) do continue; done", &mut sh, false);
    // i should reach 5 (cond fails) — step runs after continue.
    assert_eq!(sh.lookup_var("i").as_deref(), Some("5"));
}
