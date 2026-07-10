use crate::shell_state::Shell;

#[test]
fn break_in_inner_loop_exits_inner_only() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "x=0; for i in 1 2; do for j in a b; do if [ \"$j\" = \"b\" ]; then break; fi; done; x=$((x+1)); done",
        &mut sh,
        false,
    );
    // Outer loop ran both i=1 and i=2 (inner break only exits inner).
    assert_eq!(
        sh.lookup_var("x").as_deref(),
        Some("2"),
        "outer loop should run twice"
    );
    assert_eq!(
        sh.loop_depth, 0,
        "loop_depth not restored after nested-for break"
    );
}

#[test]
fn break_2_in_inner_loop_exits_both() {
    let mut sh = Shell::new();
    // Counter to verify outer loop didn't iterate again.
    let _ = crate::shell::process_line(
        "x=0; for i in 1 2; do for j in a b; do break 2; done; x=$((x+1)); done",
        &mut sh,
        false,
    );
    // x should still be 0 — break 2 exits before x=$((x+1)) runs.
    assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
}

#[test]
fn break_999_caps_in_two_loops() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "x=0; for i in 1 2; do for j in a b; do break 999; done; x=$((x+1)); done",
        &mut sh,
        false,
    );
    // Same as break 2 — cap to depth=2.
    assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
}

#[test]
fn continue_2_in_inner_loop_runs_outer_step() {
    let mut sh = Shell::new();
    // continue 2 from inner: skip rest of inner, advance outer
    let _ = crate::shell::process_line(
        "x=0; for i in 1 2 3; do for j in a; do continue 2; done; x=$((x+1)); done",
        &mut sh,
        false,
    );
    // x should be 0 — `continue 2` skips the x=... line each outer iteration.
    assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
}

#[test]
fn break_inside_function_called_from_loop_errors() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "f() { break; }; for i in 1 2; do f; done; echo done",
        &mut sh,
        false,
    );
    // The break inside f errors (loop_depth=0 inside the function);
    // for-loop continues; loop_depth is back to 0 afterward.
    assert_eq!(sh.loop_depth, 0);
}

#[test]
fn loop_depth_zero_after_loop_exits() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line("for i in 1 2 3; do :; done", &mut sh, false);
    assert_eq!(sh.loop_depth, 0);
}

#[test]
fn loop_depth_zero_after_nested_loop_exits() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "for i in 1 2; do for j in a b; do :; done; done",
        &mut sh,
        false,
    );
    assert_eq!(sh.loop_depth, 0);
}

#[test]
fn loop_depth_restored_after_function_return() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "f() { for j in a b; do :; done; }; for i in 1 2; do f; done",
        &mut sh,
        false,
    );
    // Both outer for-loop (depth +1) and inner function-then-for
    // should leave loop_depth at 0.
    assert_eq!(sh.loop_depth, 0);
}

// ----- malformed-arg break/continue: break ALL loops, terminal $? = 1 -----

#[test]
fn break_zero_breaks_all_loops_and_status_1() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "x=0; o=0; for i in 1 2; do for j in a b; do break 0; x=$((x+1)); done; o=$((o+1)); done",
        &mut sh,
        false,
    );
    // break 0 breaks ALL loops: neither the inner body after it (x) nor the
    // outer body after the inner loop (o) runs again.
    assert_eq!(
        sh.lookup_var("x").as_deref(),
        Some("0"),
        "inner body must not run after break 0"
    );
    assert_eq!(
        sh.lookup_var("o").as_deref(),
        Some("0"),
        "outer body must not run after break 0"
    );
    // The loop nest leaves $? = 1.
    assert_eq!(sh.last_status(), 1, "break 0 leaves $? = 1");
}

#[test]
fn continue_zero_breaks_all_loops_and_status_1() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "x=0; o=0; for i in 1 2; do for j in a b; do continue 0; x=$((x+1)); done; o=$((o+1)); done",
        &mut sh,
        false,
    );
    // continue 0 behaves like break-all (out-of-range), same as bash.
    assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
    assert_eq!(sh.lookup_var("o").as_deref(), Some("0"));
    assert_eq!(sh.last_status(), 1, "continue 0 leaves $? = 1");
}

#[test]
fn break_too_many_args_breaks_all_loops_and_status_1() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line(
        "x=0; o=0; for i in 1 2; do for j in a b; do break 1 2 3; x=$((x+1)); done; o=$((o+1)); done",
        &mut sh,
        false,
    );
    assert_eq!(sh.lookup_var("x").as_deref(), Some("0"));
    assert_eq!(sh.lookup_var("o").as_deref(), Some("0"));
    assert_eq!(
        sh.last_status(),
        1,
        "break with too many args leaves $? = 1"
    );
}

#[test]
fn normal_break_leaves_status_0() {
    let mut sh = Shell::new();
    let _ = crate::shell::process_line("for i in 1 2; do break; done", &mut sh, false);
    // Normal break leaves $? = 0 (no regression from the status-carrying change).
    assert_eq!(sh.last_status(), 0);
}
