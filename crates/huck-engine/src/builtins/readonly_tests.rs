use super::*;
use crate::shell_state::Shell;

fn run(shell: &mut Shell, line: &str) -> ExecOutcome {
    crate::shell::process_line(line, shell, false)
}

/// Runs a `readonly` invocation via the DeclArg strs entry point, capturing
/// stderr text (unlike the process_line-based `run` helper above, which
/// doesn't expose it) so Root C's exact error wording can be asserted.
fn run_capture_err(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out: Vec<u8> = Vec::new();
    let mut errbuf: Vec<u8> = Vec::new();
    let outcome =
        run_declaration_builtin_strs("readonly", &args_owned, &mut out, &mut errbuf, shell);
    (outcome, String::from_utf8(errbuf).unwrap())
}

#[test]
fn readonly_dash_a_with_compound_value_creates_readonly_indexed_array() {
    // Root A: `readonly -a x=(1 2)` used to hit the `invalid option` arm.
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly -a x=(1 2)");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let m = s.get_indexed("x").expect("x is an indexed array");
    assert_eq!(m.get(&0).map(String::as_str), Some("1"));
    assert_eq!(m.get(&1).map(String::as_str), Some("2"));
    assert!(s.is_readonly("x"));
    let (_, v) = s
        .iter_vars()
        .find(|(n, _)| n.as_str() == "x")
        .expect("x is set");
    assert_eq!(
        format_declare_line("x", v),
        r#"declare -ar x=([0]="1" [1]="2")"#
    );
}

#[test]
fn readonly_dash_a_single_element_compound_value() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly -a r=(7)");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let (_, v) = s
        .iter_vars()
        .find(|(n, _)| n.as_str() == "r")
        .expect("r is set");
    assert_eq!(format_declare_line("r", v), r#"declare -ar r=([0]="7")"#);
}

#[test]
fn readonly_dash_a_no_value_creates_empty_readonly_array() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly -a y");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let m = s.get_indexed("y").expect("y is an indexed array");
    assert_eq!(m.len(), 0);
    assert!(s.is_readonly("y"));
}

#[test]
fn readonly_dash_a_scalar_rhs_promotes_to_element_zero() {
    // `-a` forces even a non-compound RHS into an indexed array (bash:
    // `readonly -a s=hello` -> `declare -ar s=([0]="hello")`), matching how
    // `-A NAME=v` forces a scalar into an associative element.
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly -a s=hello");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let m = s.get_indexed("s").expect("s is an indexed array");
    assert_eq!(m.get(&0).map(String::as_str), Some("hello"));
    assert!(s.is_readonly("s"));
}

#[test]
fn readonly_dash_cap_a_regression_still_works() {
    // KEEP: readonly -A must be unaffected by the -a addition.
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly -A m=([k]=v)");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(s.get_associative("m").is_some());
    assert!(s.is_readonly("m"));
}

#[test]
fn readonly_dash_q_invalid_option_keeps_prefix() {
    // KEEP: only the readonly-variable assignment error (Root C) drops the
    // `readonly:` prefix; invalid-option errors keep it.
    let mut s = Shell::new();
    let (outcome, errtext) = run_capture_err(&["-q", "x"], &mut s);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
    assert!(
        errtext.contains("readonly: -q: invalid option"),
        "expected prefixed invalid-option error, got: {errtext:?}"
    );
}

#[test]
fn readonly_reassign_error_drops_prefix() {
    // Root C: `readonly x=1; readonly x=2` -> bash `x: readonly variable`
    // (the shell-name/line prologue stays; only the extra `readonly:`
    // builtin-name segment bash omits must be gone).
    let mut s = Shell::new();
    let (first, _) = run_capture_err(&["x=1"], &mut s);
    assert!(matches!(first, ExecOutcome::Continue(0)));
    let (second, errtext) = run_capture_err(&["x=2"], &mut s);
    assert!(matches!(second, ExecOutcome::Continue(1)));
    assert!(
        errtext.ends_with("x: readonly variable\n"),
        "expected bare 'x: readonly variable' tail, got: {errtext:?}"
    );
    assert!(
        !errtext.contains("readonly: x: readonly variable"),
        "must not carry the redundant `readonly:` builtin-name segment, got: {errtext:?}"
    );
    assert_eq!(s.lookup_var("x").as_deref(), Some("1"));
}

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

// ── v349 (#343) Task 2: Root D (quoted `name=val` is an assignment) ──────────
// and Root B (`-a`/`-A` coerces a quoted `(...)` value to an array literal).

/// Root D: a fully-quoted `'x=hi'` arg to `readonly` is an ASSIGNMENT (bash
/// applies quote removal, then sees `x=hi`), not an invalid identifier.
#[test]
fn readonly_quoted_scalar_assignment_is_an_assignment() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly 'x=hi'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(s.get("x").as_deref(), Some("hi"));
    assert!(s.is_readonly("x"));
}

/// Root D KEEP: an invalid-identifier `'3x=1'` stays a Plain arg → the
/// not-a-valid-identifier error, exit 1.
#[test]
fn readonly_quoted_invalid_ident_still_errors() {
    let mut s = Shell::new();
    let (outcome, errtext) = run_capture_err(&["3x=1"], &mut s);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(
        errtext.contains("`3x=1': not a valid identifier"),
        "{errtext:?}"
    );
    assert!(!s.is_readonly("3x"));
}

/// Root D KEEP: a bare quoted name with no `=` marks the var readonly.
#[test]
fn readonly_quoted_bare_name_marks_readonly() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly 'foo'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(s.is_readonly("foo"));
}

/// Root D contrast: WITHOUT `-a`, a quoted `(...)` scalar assigned to an
/// existing array lands as element 0 with the literal parens preserved.
#[test]
fn readonly_quoted_paren_scalar_without_dash_a_keeps_literal_parens() {
    let mut s = Shell::new();
    run(&mut s, "c=(outside)");
    let outcome = run(&mut s, "readonly 'c=(3)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let (_, v) = s.iter_vars().find(|(n, _)| n.as_str() == "c").unwrap();
    assert_eq!(format_declare_line("c", v), r#"declare -ar c=([0]="(3)")"#);
}

/// Root B: under `-a`, a quoted `(4)` scalar is coerced to an array literal
/// (element 0 = "4"), replacing the existing array.
#[test]
fn readonly_dash_a_quoted_paren_coerces_to_array_literal() {
    let mut s = Shell::new();
    run(&mut s, "d=(outside)");
    let outcome = run(&mut s, "readonly -a 'd=(4)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let (_, v) = s.iter_vars().find(|(n, _)| n.as_str() == "d").unwrap();
    assert_eq!(format_declare_line("d", v), r#"declare -ar d=([0]="4")"#);
    assert!(s.is_readonly("d"));
}

/// Root B: multi-element quoted array literal parses fully under `-a`.
#[test]
fn readonly_dash_a_quoted_multi_element_array_literal() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly -a 'd=(4 5)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let m = s.get_indexed("d").expect("d indexed");
    assert_eq!(m.get(&0).map(String::as_str), Some("4"));
    assert_eq!(m.get(&1).map(String::as_str), Some("5"));
}

/// Root B: `readonly -a r='(7)'` (quoted RHS via AssignPrefix) coerces too.
#[test]
fn readonly_dash_a_quoted_rhs_paren_coerces() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "readonly -a r='(7)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let (_, v) = s.iter_vars().find(|(n, _)| n.as_str() == "r").unwrap();
    assert_eq!(format_declare_line("r", v), r#"declare -ar r=([0]="7")"#);
}

/// Root B (export): `export -a r='(7)'` coerces the quoted `(...)` value.
#[test]
fn export_dash_a_quoted_paren_coerces_to_array() {
    let mut s = Shell::new();
    run(&mut s, "r=(1)");
    let outcome = run(&mut s, "export -a r='(7)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let (_, v) = s.iter_vars().find(|(n, _)| n.as_str() == "r").unwrap();
    assert_eq!(format_declare_line("r", v), r#"declare -ax r=([0]="7")"#);
}

/// Root B negative: `export r='(5)'` WITHOUT `-a` keeps the literal scalar.
#[test]
fn export_quoted_paren_without_dash_a_keeps_scalar() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "export r='(5)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let (_, v) = s.iter_vars().find(|(n, _)| n.as_str() == "r").unwrap();
    assert_eq!(format_declare_line("r", v), r#"declare -x r="(5)""#);
}

/// Root B (declare): `declare -a 'x=(1 2)'` coerces the quoted `(...)` value
/// to an array literal and sets the array attribute.
#[test]
fn declare_dash_a_quoted_paren_coerces_to_array() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "declare -a 'x=(1 2)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let (_, v) = s.iter_vars().find(|(n, _)| n.as_str() == "x").unwrap();
    assert_eq!(
        format_declare_line("x", v),
        r#"declare -a x=([0]="1" [1]="2")"#
    );
}

/// Root B (declare -A): quoted associative literal coerces under `-A`.
#[test]
fn declare_dash_aa_quoted_assoc_literal_coerces() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "declare -A 'am=([k]=v)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let m = s.get_associative("am").expect("am associative");
    assert_eq!(m.get("k"), Some("v"));
}

/// Root B negative (declare): WITHOUT `-a`, a quoted `(3)` stays a literal
/// scalar.
#[test]
fn declare_quoted_paren_without_dash_a_keeps_scalar() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "declare 'p=(3)'");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let (_, v) = s.iter_vars().find(|(n, _)| n.as_str() == "p").unwrap();
    assert_eq!(format_declare_line("p", v), r#"declare -- p="(3)""#);
}

/// Root B (local): `local -a 'y=(3 4)'` coerces the quoted `(...)` value.
#[test]
fn local_dash_a_quoted_paren_coerces_to_array() {
    // `y` is local to `f`, so copy it into a global to assert the coercion
    // actually produced a 2-element indexed array (not a scalar).
    let mut s = Shell::new();
    let outcome = run(&mut s, r#"f(){ local -a 'y=(3 4)'; g=("${y[@]}"); }; f"#);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let m = s.get_indexed("g").expect("g is an indexed array");
    assert_eq!(m.get(&0).map(String::as_str), Some("3"));
    assert_eq!(m.get(&1).map(String::as_str), Some("4"));
}
