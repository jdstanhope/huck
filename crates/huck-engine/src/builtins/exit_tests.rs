use super::*;
use crate::shell_state::Shell;

#[test]
fn exit_no_args_inherits_last_status() {
    let mut shell = Shell::new();
    shell.set_last_status(42);
    let outcome = builtin_exit(&[], &mut std::io::stderr(), &shell);
    assert!(matches!(outcome, ExecOutcome::Exit(42)));
}

#[test]
fn exit_no_args_inherits_zero_when_clean() {
    let shell = Shell::new();
    let outcome = builtin_exit(&[], &mut std::io::stderr(), &shell);
    assert!(matches!(outcome, ExecOutcome::Exit(0)));
}
