use super::*;
use crate::shell_state::Shell;

#[test]
fn source_no_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(".", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn source_missing_file_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        ".",
        &["/nonexistent/file/path/huck-v51-test".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn source_depth_limit_errors_status_1() {
    let mut shell = Shell::new();
    shell.source_depth = 64;
    let mut buf: Vec<u8> = Vec::new();
    // Use a path that would otherwise resolve fine — depth check
    // fires before the path resolution.
    let outcome = run_builtin(
        ".",
        &["/etc/hostname".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    // Counter unchanged because the early return bypasses the
    // increment.
    assert_eq!(shell.source_depth, 64);
}

#[test]
fn is_builtin_recognises_dot_and_source() {
    assert!(is_builtin("."));
    assert!(is_builtin("source"));
}

#[test]
fn is_special_builtin_includes_dot_and_source() {
    assert!(is_special_builtin("."));
    assert!(is_special_builtin("source"));
}
