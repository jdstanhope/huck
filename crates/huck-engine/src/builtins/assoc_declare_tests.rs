use super::*;
use crate::shell_state::Shell;

fn run(shell: &mut Shell, line: &str) -> ExecOutcome {
    crate::shell::process_line(line, shell, false)
}

#[test]
fn declare_dash_cap_a_creates_empty_associative() {
    let mut s = Shell::new();
    let _ = run(&mut s, "declare -A m");
    assert!(s.get_associative("m").is_some());
    assert_eq!(s.get_associative("m").unwrap().len(), 0);
}

#[test]
fn declare_dash_cap_a_with_value() {
    let mut s = Shell::new();
    let _ = run(&mut s, "declare -A m=([foo]=bar [baz]=qux)");
    assert_eq!(s.lookup_associative_element("m", "foo"), Some("bar".into()));
    assert_eq!(s.lookup_associative_element("m", "baz"), Some("qux".into()));
}

#[test]
fn declare_p_formats_associative() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    s.set_associative_element("m", "k1".into(), "v1".into())
        .unwrap();
    s.set_associative_element("m", "k2".into(), "v2".into())
        .unwrap();
    let v = s.iter_vars().find(|(n, _)| n.as_str() == "m").unwrap().1;
    let line = format_declare_line("m", v);
    assert_eq!(line, r#"declare -A m=([k1]="v1" [k2]="v2" )"#);
}

#[test]
fn declare_dash_cap_a_i_creates_integer_assoc() {
    // L-49: `declare -Ai` creates an integer-flagged associative array
    // whose VALUES arith-coerce on assignment (keys are not coerced).
    let mut s = Shell::new();
    let outcome = run(&mut s, "declare -Ai m=([x]=2+3 [y]=10)");
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(s.is_integer("m"));
    let pairs = s.get_associative("m").unwrap();
    assert_eq!(
        pairs
            .iter()
            .find(|(k, _)| k == "x")
            .map(|(_, v)| v.as_str()),
        Some("5")
    );
    assert_eq!(
        pairs
            .iter()
            .find(|(k, _)| k == "y")
            .map(|(_, v)| v.as_str()),
        Some("10")
    );
}

#[test]
fn declare_dash_a_cap_a_errors() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "declare -aA m");
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn declare_dash_cap_a_on_existing_indexed_errors() {
    let mut s = Shell::new();
    let _ = run(&mut s, "a=(x y z)");
    let outcome = run(&mut s, "declare -A a");
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(s.get_indexed("a").is_some());
    assert!(s.get_associative("a").is_none());
}

#[test]
fn declare_dash_cap_a_on_existing_scalar_errors() {
    let mut s = Shell::new();
    let _ = run(&mut s, "s=hello");
    let outcome = run(&mut s, "declare -A s");
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn readonly_dash_cap_a_creates_readonly_associative() {
    let mut s = Shell::new();
    let _ = run(&mut s, "readonly -A m=([k]=v)");
    assert!(s.get_associative("m").is_some());
    let _ = run(&mut s, "m[k2]=v2");
    assert!(s.lookup_associative_element("m", "k2").is_none());
}

#[test]
fn export_associative_rejects() {
    let mut s = Shell::new();
    let outcome = run(&mut s, "export m=([k]=v)");
    assert!(matches!(
        outcome,
        ExecOutcome::Continue(1) | ExecOutcome::Exit(1)
    ));
    assert!(s.get_associative("m").is_none());
}
