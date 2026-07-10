use super::*;
use crate::shell_state::Shell;

#[test]
fn readonly_with_value_sets_and_locks() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["X=hi".to_string()];
    let outcome = run_declaration_builtin_strs(
        "readonly",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X").as_deref(), Some("hi"));
    assert!(shell.is_readonly("X"));
}

#[test]
fn readonly_no_value_creates_empty_and_locks() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["X".to_string()];
    let outcome = run_declaration_builtin_strs(
        "readonly",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X").as_deref(), Some(""));
    assert!(shell.is_readonly("X"));
}

#[test]
fn readonly_no_value_keeps_existing_value() {
    let mut shell = Shell::new();
    shell.set("X", "prev".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["X".to_string()];
    let outcome = run_declaration_builtin_strs(
        "readonly",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X").as_deref(), Some("prev"));
    assert!(shell.is_readonly("X"));
}

#[test]
fn readonly_multi_arg_mixed_forms() {
    let mut shell = Shell::new();
    shell.set("B", "had".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["A=1".to_string(), "B".to_string(), "C=3".to_string()];
    let outcome = run_declaration_builtin_strs(
        "readonly",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("A").as_deref(), Some("1"));
    assert_eq!(shell.lookup_var("B").as_deref(), Some("had"));
    assert_eq!(shell.lookup_var("C").as_deref(), Some("3"));
    assert!(shell.is_readonly("A"));
    assert!(shell.is_readonly("B"));
    assert!(shell.is_readonly("C"));
}

#[test]
fn readonly_invalid_identifier_errors() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["1foo=bar".to_string()];
    let outcome = run_declaration_builtin_strs(
        "readonly",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(shell.lookup_var("1foo").is_none());
}

#[test]
fn readonly_listing_no_args() {
    let mut shell = Shell::new();
    shell.set("X", "v".to_string());
    shell.mark_readonly("X");
    shell.set("Y", "w".to_string());
    shell.mark_readonly("Y");
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "readonly",
        &[],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    // declare -p style listing; scalars render with `-r` attrs.
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&r#"declare -r X="v""#));
    assert!(lines.contains(&r#"declare -r Y="w""#));
}

#[test]
fn readonly_dash_p_same_as_no_args() {
    let mut shell = Shell::new();
    shell.set("X", "v".to_string());
    shell.mark_readonly("X");
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "readonly",
        &["-p".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.lines().any(|l| l == r#"declare -r X="v""#));
}

#[test]
fn readonly_overwrite_existing_readonly_errors() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    run_declaration_builtin_strs(
        "readonly",
        &["X=first".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    let outcome = run_declaration_builtin_strs(
        "readonly",
        &["X=second".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert_eq!(shell.lookup_var("X").as_deref(), Some("first"));
    assert!(shell.is_readonly("X"));
}

#[test]
fn unset_readonly_errors_status_1() {
    let mut shell = Shell::new();
    shell.set("X", "v".to_string());
    shell.mark_readonly("X");
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "unset",
        &["X".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
}

#[test]
fn export_readonly_value_errors_but_bare_export_succeeds() {
    let mut shell = Shell::new();
    shell.set("X", "v".to_string());
    shell.mark_readonly("X");
    let mut buf: Vec<u8> = Vec::new();
    // `export X=newval` should error and not overwrite.
    let bad = run_declaration_builtin_strs(
        "export",
        &["X=newval".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(bad, ExecOutcome::Continue(1)));
    assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
    // `export X` (bare) should succeed and flip the export flag.
    let bare = run_declaration_builtin_strs(
        "export",
        &["X".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(bare, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X").as_deref(), Some("v"));
    assert!(shell.is_readonly("X"));
}

#[test]
fn export_set_preserves_readonly_flag_on_existing_var() {
    // Regression: export_set must not silently strip the readonly
    // flag on an already-present Variable. Without the fix, a
    // future Task 2 caller (apply_inline_assignments) that bypasses
    // the is_readonly check would clobber readonly state.
    let mut shell = Shell::new();
    shell.set("X", "outer".to_string());
    shell.mark_readonly("X");
    // Direct call to export_set on an already-readonly var.
    shell.export_set("X", "new".to_string());
    // Value updated, but readonly flag must stay set.
    assert!(shell.is_readonly("X"));
}
