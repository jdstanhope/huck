use super::*;
use crate::shell_state::Shell;

#[test]
fn colon_exits_zero() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(":", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn colon_with_args_exits_zero() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["one".to_string(), "two".to_string()];
    let outcome = run_builtin(":", &args, &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}
