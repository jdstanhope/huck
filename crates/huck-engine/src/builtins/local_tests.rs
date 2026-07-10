use super::*;
use crate::shell_state::Shell;

#[test]
fn local_outside_function_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    // local_scopes is empty (we never pushed a frame).
    let outcome = run_declaration_builtin_strs(
        "local",
        &["X=hi".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn local_with_value_sets_and_records_snapshot() {
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["XYZ_LOCAL_T1=hi".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("XYZ_LOCAL_T1").as_deref(), Some("hi"));
    // Snapshot recorded: X was unset before, so snapshot is None.
    let frame = shell.local_scopes.last().unwrap();
    assert!(frame.contains_key("XYZ_LOCAL_T1"));
    assert!(frame["XYZ_LOCAL_T1"].is_none());
}

#[test]
fn local_without_value_leaves_unset() {
    // Bare `local NAME` declares the var function-local but UNSET, matching
    // bash (verified: `f(){ local x; [[ -v x ]] && echo S || echo U; }; f`
    // prints `U`). It used to be set-empty; that was the M-111 bug.
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["XYZ_LOCAL_T2".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("XYZ_LOCAL_T2").as_deref(), None);
}

#[test]
fn local_snapshots_existing_var() {
    let mut shell = Shell::new();
    shell.set("XYZ_LOCAL_T3", "outer".to_string());
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["XYZ_LOCAL_T3=inner".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    // After `local`, the var has the inner value.
    assert_eq!(shell.lookup_var("XYZ_LOCAL_T3").as_deref(), Some("inner"));
    // The frame holds the snapshot of the outer value.
    let snapshot = shell
        .local_scopes
        .last()
        .unwrap()
        .get("XYZ_LOCAL_T3")
        .cloned()
        .unwrap();
    let v = snapshot.expect("expected Some snapshot for previously-set var");
    assert!(matches!(&v.value, crate::shell_state::VarValue::Scalar(s) if s == "outer"));
}

#[test]
fn local_idempotent_in_same_frame() {
    let mut shell = Shell::new();
    shell.set("XYZ_LOCAL_T4", "outer".to_string());
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    // First `local`: snapshot the outer value.
    let _ = run_declaration_builtin_strs(
        "local",
        &["XYZ_LOCAL_T4=first".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    // Second `local` for the same name in the same frame: must NOT
    // re-snapshot (otherwise it would overwrite the outer snapshot
    // with "first").
    let _ = run_declaration_builtin_strs(
        "local",
        &["XYZ_LOCAL_T4=second".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    // Current value reflects the second assignment.
    assert_eq!(shell.lookup_var("XYZ_LOCAL_T4").as_deref(), Some("second"));
    // Snapshot still holds the original outer value.
    let snapshot = shell
        .local_scopes
        .last()
        .unwrap()
        .get("XYZ_LOCAL_T4")
        .cloned()
        .unwrap();
    let v = snapshot.expect("expected Some outer snapshot");
    assert!(matches!(&v.value, crate::shell_state::VarValue::Scalar(s) if s == "outer"));
}

#[test]
fn local_invalid_identifier_errors() {
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["1foo=bar".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn local_dash_i_marks_integer_and_coerces_rhs() {
    // `local -i x=3+4` evaluates the RHS arithmetically (→ 7) and flags
    // the local integer, like `declare -i`.
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["-i".to_string(), "XYZ_LI=3+4".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("XYZ_LI").as_deref(), Some("7"));
    assert!(shell.is_integer("XYZ_LI"));
}

#[test]
fn local_dash_i_bare_then_assign_coerces() {
    // `local -i x` followed by `x=2+3` coerces on assignment (→ 5).
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let _ = run_declaration_builtin_strs(
        "local",
        &["-i".to_string(), "XYZ_LIB".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(shell.is_integer("XYZ_LIB"));
    let _ = shell.try_set("XYZ_LIB", "2+3".to_string());
    assert_eq!(shell.lookup_var("XYZ_LIB").as_deref(), Some("5"));
}

#[test]
fn local_dash_r_marks_readonly() {
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["-r".to_string(), "XYZ_LR=fixed".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("XYZ_LR").as_deref(), Some("fixed"));
    assert!(shell.is_readonly("XYZ_LR"));
}

#[test]
fn local_clustered_ri_applies_both_attrs() {
    // `local -ri n=5+5`: integer (→ 10) AND readonly.
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["-ri".to_string(), "XYZ_LRI=5+5".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("XYZ_LRI").as_deref(), Some("10"));
    assert!(shell.is_integer("XYZ_LRI"));
    assert!(shell.is_readonly("XYZ_LRI"));
}

#[test]
fn local_nameref_invalid_target_errors() {
    // `local -n XYZ_LU=1` — target "1" is not a valid identifier → rc 1.
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["-n".to_string(), "XYZ_LU=1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn local_case_fold_lower_upper() {
    // `local -l v=HELLO` folds to lowercase.
    let mut shell = Shell::new();
    shell.local_scopes.push(std::collections::HashMap::new());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "local",
        &["-l".to_string(), "V=HELLO".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("V").as_deref(), Some("hello"));

    // `local -u v=hello` folds to uppercase.
    let mut shell2 = Shell::new();
    shell2.local_scopes.push(std::collections::HashMap::new());
    let mut buf2: Vec<u8> = Vec::new();
    let outcome2 = run_declaration_builtin_strs(
        "local",
        &["-u".to_string(), "W=hello".to_string()],
        &mut buf2,
        &mut std::io::stderr(),
        &mut shell2,
    );
    assert!(matches!(outcome2, ExecOutcome::Continue(0)));
    assert_eq!(shell2.lookup_var("W").as_deref(), Some("HELLO"));
}
