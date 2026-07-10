use super::*;
use crate::shell_state::Shell;

#[test]
fn set_no_args_lists_sorted_vars() {
    let mut shell = Shell::new();
    // Use unique names unlikely to collide with environment.
    shell.set("ZZTEST_C", "three".to_string());
    shell.set("ZZTEST_A", "one".to_string());
    shell.set("ZZTEST_B", "two".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("set", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    // Find the three target lines and confirm they appear in
    // sorted order relative to each other.
    let a_idx = out.find("ZZTEST_A=").expect("missing A");
    let b_idx = out.find("ZZTEST_B=").expect("missing B");
    let c_idx = out.find("ZZTEST_C=").expect("missing C");
    assert!(a_idx < b_idx, "A should come before B");
    assert!(b_idx < c_idx, "B should come before C");
    // Format check: a plain alphanumeric value is printed BARE (no quotes),
    // matching bash's `set` listing (only metacharacter values get quoted).
    assert!(
        out.contains("ZZTEST_A=one\n"),
        "expected bare value: {out:?}"
    );
}

#[test]
fn set_escape_value_matches_bash_listing_quoting() {
    // bash `set` (no args) value quoting — verified against bash 5.2.
    assert_eq!(set_escape_value("abc"), "abc"); // plain → bare
    assert_eq!(set_escape_value("123"), "123"); // digits → bare
    assert_eq!(set_escape_value("ab#cd"), "ab#cd"); // # not leading → bare
    assert_eq!(set_escape_value("#abc"), "'#abc'"); // # leading → quoted
    assert_eq!(set_escape_value("a~b"), "a~b"); // ~ not leading → bare
    assert_eq!(set_escape_value("~"), "'~'"); // ~ leading → quoted
    assert_eq!(set_escape_value("x=~"), "'x=~'"); // ~ after = → quoted
    assert_eq!(set_escape_value("a b"), "'a b'"); // space → quoted
    assert_eq!(set_escape_value("a*b"), "'a*b'"); // glob → quoted
    assert_eq!(set_escape_value(""), ""); // empty → bare (name=)
    assert_eq!(set_escape_value("'"), r"\'"); // lone quote → \'
    assert_eq!(set_escape_value("a'b"), r"'a'\''b'"); // embedded quote
    assert_eq!(set_escape_value("''"), r"''\'''\'''"); // two quotes → wrap
    assert_eq!(set_escape_value("\t"), "$'\\t'"); // control → ANSI-C
}

#[test]
fn set_double_dash_alone_clears_positional() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string()];
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "set",
        &["--".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(shell.positional_args.is_empty());
}

#[test]
fn set_double_dash_with_args_replaces() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "set",
        &["--".to_string(), "one".to_string(), "two".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.positional_args, vec!["one", "two"]);
}

#[test]
fn set_bare_args_replaces_positional() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "set",
        &["one".to_string(), "two".to_string(), "three".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.positional_args, vec!["one", "two", "three"]);
}

#[test]
fn set_dash_x_enables_xtrace() {
    // -x (xtrace) implemented in v103.
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "set",
        &["-x".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.xtrace);
}

#[test]
fn set_plus_x_disables_xtrace() {
    let mut shell = Shell::new();
    shell.shell_options.xtrace = true;
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "set",
        &["+x".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(!shell.shell_options.xtrace);
}
