use super::*;
use crate::shell_state::Shell;

fn run(shell: &mut Shell, line: &str) -> ExecOutcome {
    crate::shell::process_line(line, shell, false)
}

#[test]
fn declare_dash_a_creates_empty_array() {
    let mut s = Shell::new();
    let _ = run(&mut s, "declare -a a");
    assert!(s.get_indexed("a").is_some());
    assert_eq!(s.get_indexed("a").unwrap().len(), 0);
}

#[test]
fn declare_dash_a_with_value() {
    let mut s = Shell::new();
    let _ = run(&mut s, "declare -a a=(x y)");
    let m = s.get_indexed("a").unwrap();
    assert_eq!(m.get(&0).map(String::as_str), Some("x"));
    assert_eq!(m.get(&1).map(String::as_str), Some("y"));
}

#[test]
fn declare_p_formats_array() {
    let mut s = Shell::new();
    let _ = run(&mut s, "a=(x y)");
    let (_, v) = s
        .iter_vars()
        .find(|(n, _)| n.as_str() == "a")
        .expect("a is set");
    let line = format_declare_line("a", v);
    assert_eq!(line, r#"declare -a a=([0]="x" [1]="y")"#);
}

#[test]
fn declare_dash_ai_creates_integer_array() {
    // L-49: `declare -ai` now creates an integer-flagged indexed array
    // whose element values arith-coerce on assignment.
    let mut s = Shell::new();
    let outcome = run(&mut s, "declare -ai a=(2+3 4*5)");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(s.is_integer("a"));
    let m = s.get_indexed("a").unwrap();
    assert_eq!(m.get(&0).map(String::as_str), Some("5"));
    assert_eq!(m.get(&1).map(String::as_str), Some("20"));
}

#[test]
fn readonly_array_blocks_element_write() {
    let mut s = Shell::new();
    let _ = run(&mut s, "readonly a=(x y)");
    let _ = run(&mut s, "a[2]=z");
    let m = s.get_indexed("a").unwrap();
    assert!(m.get(&2).is_none());
}

#[test]
fn export_array_assigns_and_exports() {
    // #82: bash accepts `export a=(x y)` — assign the indexed array AND mark it
    // exported (declare -ax), rc 0. huck used to reject with "cannot export
    // arrays" and not create the variable.
    let mut s = Shell::new();
    let outcome = run(&mut s, "export a=(x y)");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let m = s.get_indexed("a").expect("array a created");
    assert_eq!(m.get(&0).map(String::as_str), Some("x"));
    assert_eq!(m.get(&1).map(String::as_str), Some("y"));
    let (_, v) = s
        .iter_vars()
        .find(|(n, _)| n.as_str() == "a")
        .expect("a is set");
    assert!(v.exported, "a must carry the export attribute");
    assert!(matches!(v.value, crate::shell_state::VarValue::Indexed(_)));
}

#[test]
fn exported_array_omitted_from_child_env_but_scalar_kept() {
    // #28: bash never puts an array into a child's environment; huck used to
    // leak the [0] element as a scalar. An exported scalar is still inherited.
    let mut s = Shell::new();
    let _ = run(&mut s, "export a=(x y z)");
    let _ = run(&mut s, "export s=hi");
    assert!(
        !s.exported_env().any(|(k, _)| k == "a"),
        "exported array must NOT appear in the child environment"
    );
    assert!(
        s.exported_env().any(|(k, v)| k == "s" && v == "hi"),
        "exported scalar must still be inherited"
    );
}

#[test]
fn readonly_p_lists_array_with_full_elements() {
    // Regression: `readonly -p` used to route through scalar_view and
    // collapse arrays to element 0. The fix routes through
    // format_declare_line so all elements survive.
    let mut s = Shell::new();
    let _ = run(&mut s, "readonly a=(x y z)");
    let (_, v) = s
        .iter_vars()
        .find(|(n, _)| n.as_str() == "a")
        .expect("a is set");
    let line = format_declare_line("a", v);
    assert_eq!(line, r#"declare -ar a=([0]="x" [1]="y" [2]="z")"#);

    // Also exercise the dispatched listing path end-to-end so we
    // don't drift on the writeln formatting.
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin(
        "readonly",
        &[DeclArg::Plain("-p".to_string())],
        &mut buf,
        &mut std::io::stderr(),
        &mut s,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert!(
        out.lines()
            .any(|l| l == r#"declare -ar a=([0]="x" [1]="y" [2]="z")"#),
        "stdout: {out:?}",
    );
}
