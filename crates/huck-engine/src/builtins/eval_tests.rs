use super::*;
use crate::shell_state::Shell;

fn run(args: &[&str], shell: &mut Shell) -> ExecOutcome {
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut buf: Vec<u8> = Vec::new();
    run_builtin("eval", &args_owned, &mut buf, &mut std::io::stderr(), shell)
}

#[test]
fn eval_no_args_exits_zero() {
    let mut shell = Shell::new();
    assert!(matches!(run(&[], &mut shell), ExecOutcome::Continue(0)));
}

#[test]
fn eval_empty_arg_exits_zero() {
    let mut shell = Shell::new();
    assert!(matches!(run(&[""], &mut shell), ExecOutcome::Continue(0)));
}

#[test]
fn eval_simple_command_runs() {
    let mut shell = Shell::new();
    // process_line writes to process stdout (not the builtin's
    // `out` writer), so assert the side effect on shell state.
    let oc = run(&["X_EVAL_T3=hello"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_EVAL_T3").as_deref(), Some("hello"));
}

#[test]
fn eval_assignment_persists() {
    let mut shell = Shell::new();
    let oc = run(&["X_EVAL_T4=42"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_EVAL_T4").as_deref(), Some("42"));
}

#[test]
fn eval_false_returns_one() {
    let mut shell = Shell::new();
    let oc = run(&["false"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn eval_exit_propagates() {
    let mut shell = Shell::new();
    let oc = run(&["exit", "7"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Exit(7)));
}
