use super::*;
use crate::shell_state::Shell;

#[test]
fn true_exits_zero() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("true", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn false_exits_one() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("false", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn true_and_false_ignore_args() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["ignored".to_string()];
    let t = run_builtin("true", &args, &mut buf, &mut std::io::stderr(), &mut shell);
    let f = run_builtin("false", &args, &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(t, ExecOutcome::Continue(0)));
    assert!(matches!(f, ExecOutcome::Continue(1)));
}
