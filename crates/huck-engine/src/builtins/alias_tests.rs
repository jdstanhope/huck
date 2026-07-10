use super::*;
use crate::shell_state::Shell;

#[test]
fn alias_no_args_lists_empty() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("alias", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(
        buf.is_empty(),
        "expected empty output, got {:?}",
        String::from_utf8_lossy(&buf)
    );
}

#[test]
fn alias_no_args_lists_sorted() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    shell.aliases.insert("la".to_string(), "ls -A".to_string());
    shell.aliases.insert("l".to_string(), "ls".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("alias", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        vec!["alias l='ls'", "alias la='ls -A'", "alias ll='ls -l'",]
    );
}

#[test]
fn alias_defines_simple() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "alias",
        &["ll=ls -l".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.aliases.get("ll").map(|s| s.as_str()), Some("ls -l"));
}

#[test]
fn alias_lookup_existing_prints() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "alias",
        &["ll".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out, "alias ll='ls -l'\n");
}

#[test]
fn alias_lookup_missing_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "alias",
        &["xyz".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn unalias_removes_existing() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "unalias",
        &["ll".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(!shell.aliases.contains_key("ll"));
}

#[test]
fn unalias_missing_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "unalias",
        &["xyz".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn unalias_dash_a_clears_all() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    shell.aliases.insert("la".to_string(), "ls -A".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "unalias",
        &["-a".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(shell.aliases.is_empty());
}

#[test]
fn unalias_no_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("unalias", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}
