use super::*;
use crate::shell_state::Shell;

// ── try_set integer-eval ────────────────────────────────

#[test]
fn try_set_non_integer_passes_through() {
    let mut shell = Shell::new();
    assert!(shell.try_set("X_INT_T1", "2+3".to_string()).is_ok());
    assert_eq!(shell.lookup_var("X_INT_T1").as_deref(), Some("2+3"));
}

#[test]
fn try_set_integer_simple_arith() {
    let mut shell = Shell::new();
    shell.mark_integer("X_INT_T2");
    assert!(shell.try_set("X_INT_T2", "2+3".to_string()).is_ok());
    assert_eq!(shell.lookup_var("X_INT_T2").as_deref(), Some("5"));
}

#[test]
fn try_set_integer_negative_result() {
    let mut shell = Shell::new();
    shell.mark_integer("X_INT_T3");
    assert!(shell.try_set("X_INT_T3", "0-5".to_string()).is_ok());
    assert_eq!(shell.lookup_var("X_INT_T3").as_deref(), Some("-5"));
}

#[test]
fn try_set_integer_invalid_silently_zero() {
    let mut shell = Shell::new();
    shell.mark_integer("X_INT_T4");
    assert!(shell.try_set("X_INT_T4", "abc".to_string()).is_ok());
    assert_eq!(shell.lookup_var("X_INT_T4").as_deref(), Some("0"));
}

#[test]
fn try_set_integer_with_var_ref() {
    let mut shell = Shell::new();
    shell.set("Y_INT_T5", "10".to_string());
    shell.mark_integer("X_INT_T5");
    assert!(shell.try_set("X_INT_T5", "Y_INT_T5*2".to_string()).is_ok());
    assert_eq!(shell.lookup_var("X_INT_T5").as_deref(), Some("20"));
}

#[test]
fn try_set_readonly_checked_before_integer() {
    let mut shell = Shell::new();
    shell.set("X_INT_T6", "outer".to_string());
    shell.mark_readonly("X_INT_T6");
    shell.mark_integer("X_INT_T6");
    // try_set must return Err on readonly; value should NOT
    // change to "5".
    assert!(shell.try_set("X_INT_T6", "5".to_string()).is_err());
    assert_eq!(shell.lookup_var("X_INT_T6").as_deref(), Some("outer"));
}

// ── builtin_declare wiring ──────────────────────────────

fn run_declare(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_declaration_builtin_strs(
        "declare",
        &args_owned,
        &mut buf,
        &mut std::io::stderr(),
        shell,
    );
    (outcome, String::from_utf8(buf).unwrap())
}

#[test]
fn declare_i_marks_and_evals() {
    let mut shell = Shell::new();
    let (oc, _) = run_declare(&["-i", "X_INT_D1=2+3"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_INT_D1").as_deref(), Some("5"));
    assert!(shell.is_integer("X_INT_D1"));
}

#[test]
fn declare_plus_i_unmarks() {
    let mut shell = Shell::new();
    run_declare(&["-i", "X_INT_D2=5"], &mut shell);
    assert!(shell.is_integer("X_INT_D2"));
    let (oc, _) = run_declare(&["+i", "X_INT_D2"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.is_integer("X_INT_D2"));
    // Value preserved.
    assert_eq!(shell.lookup_var("X_INT_D2").as_deref(), Some("5"));
}

#[test]
fn declare_i_existing_var_no_reeval() {
    let mut shell = Shell::new();
    shell.set("X_INT_D3", "2+3".to_string());
    let (oc, _) = run_declare(&["-i", "X_INT_D3"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    // Value preserved; no re-eval on flag set without =.
    assert_eq!(shell.lookup_var("X_INT_D3").as_deref(), Some("2+3"));
    assert!(shell.is_integer("X_INT_D3"));
}

#[test]
fn declare_i_on_readonly_errors() {
    let mut shell = Shell::new();
    shell.set("X_INT_D4", "outer".to_string());
    shell.mark_readonly("X_INT_D4");
    let (oc, _) = run_declare(&["-i", "X_INT_D4"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    // Integer flag NOT set on a readonly var.
    assert!(!shell.is_integer("X_INT_D4"));
}

#[test]
fn declare_ri_on_readonly_errors_without_corrupting_attrs() {
    // Regression: previously `declare -ri X=5` on already-readonly X
    // skipped the integer-readonly guard because want_readonly was
    // also true, then mark_integer ran before the inner -r readonly
    // check fired. Result: the variable's integer flag was set even
    // though the command errored. Bash leaves attributes unchanged
    // when the declare fails.
    let mut shell = Shell::new();
    shell.set("X_INT_D5", "outer".to_string());
    shell.mark_readonly("X_INT_D5");
    let (oc, _) = run_declare(&["-ri", "X_INT_D5=5"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    // Integer flag must NOT be set; value unchanged.
    assert!(!shell.is_integer("X_INT_D5"));
    assert_eq!(shell.lookup_var("X_INT_D5").as_deref(), Some("outer"));
}
