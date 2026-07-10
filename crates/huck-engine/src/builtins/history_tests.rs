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
    let outcome = run_builtin(
        "history",
        &["--bogus".to_string()],
        &mut out,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}
