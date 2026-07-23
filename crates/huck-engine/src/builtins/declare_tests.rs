use super::*;
use crate::shell_state::Shell;

fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
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

fn run_typeset(args: &[&str], shell: &mut Shell) -> ExecOutcome {
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut buf: Vec<u8> = Vec::new();
    run_declaration_builtin_strs(
        "typeset",
        &args_owned,
        &mut buf,
        &mut std::io::stderr(),
        shell,
    )
}

#[test]
fn declare_bare_sets_var() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["X_DECL=hi"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_DECL").as_deref(), Some("hi"));
}

#[test]
fn declare_r_sets_and_locks() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-r", "X_DECL_R=hi"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_DECL_R").as_deref(), Some("hi"));
    assert!(shell.is_readonly("X_DECL_R"));
}

#[test]
fn declare_x_sets_and_exports() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-x", "X_DECL_X=hi"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_DECL_X").as_deref(), Some("hi"));
    assert!(shell.is_exported("X_DECL_X"));
}

#[test]
fn declare_rx_combines() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-rx", "X_DECL_RX=hi"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.is_readonly("X_DECL_RX"));
    assert!(shell.is_exported("X_DECL_RX"));
}

#[test]
fn declare_plus_x_unexports() {
    let mut shell = Shell::new();
    shell.export_set("X_DECL_UNEX", "v".to_string());
    assert!(shell.is_exported("X_DECL_UNEX"));
    let (oc, _) = run(&["+x", "X_DECL_UNEX"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_DECL_UNEX").as_deref(), Some("v"));
    assert!(!shell.is_exported("X_DECL_UNEX"));
}

#[test]
fn declare_plus_r_errors() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["+r", "X_FOO"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn declare_p_prints_known_var() {
    let mut shell = Shell::new();
    shell.set("X_DECL_P", "hi".to_string());
    let (oc, out) = run(&["-p", "X_DECL_P"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out, "declare -- X_DECL_P=\"hi\"\n");
}

#[test]
fn declare_p_missing_errors() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-p", "X_DECL_MISSING"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn declare_f_lists_functions() {
    let mut shell = Shell::new();
    let body = Box::new(crate::command::Command::Simple(
        crate::command::SimpleCommand::Assign(vec![], 0),
    ));
    shell.define_function("fn1".to_string(), body.clone(), 0);
    shell.define_function("fn2".to_string(), body, 0);
    let (oc, out) = run(&["-f"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    // v146: `-f` prints the normalized function body via `generate`, so
    // each function shows its `NAME ()` header (not the old `declare -f`
    // stub line). Sorted; both present.
    assert!(out.contains("fn1 ()"), "got {out:?}");
    assert!(out.contains("fn2 ()"), "got {out:?}");
    assert!(
        out.find("fn1").unwrap() < out.find("fn2").unwrap(),
        "expected sorted; got {out:?}",
    );
}

#[test]
fn declare_f_missing_is_silent() {
    // bash: `declare -f`/`-F` on a missing function emits nothing on
    // stdout and returns rc 1 (the "not found" stderr line is gone).
    let mut shell = Shell::new();
    let (oc, out) = run(&["-f", "nope"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    assert_eq!(out, "");
}

#[test]
fn declare_f_named_function_found() {
    let mut shell = Shell::new();
    let body = Box::new(crate::command::Command::Simple(
        crate::command::SimpleCommand::Assign(vec![], 0),
    ));
    shell.define_function("fn1".to_string(), body, 0);
    let (oc, out) = run(&["-F", "fn1"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    // bash `declare -F NAME` (explicit name) → bare name (not "declare -f NAME").
    assert_eq!(out, "fn1\n");
}

#[cfg(test)]
fn define_fn(shell: &mut Shell, src: &str) {
    let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        src,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .unwrap()
    .unwrap();
    let crate::command::Command::FunctionDef { name, body, .. } = seq.first else {
        panic!("expected function def")
    };
    shell.define_function(name, body, 0);
}

#[test]
fn declare_dash_f_explicit_name_is_bare() {
    let mut shell = Shell::new();
    define_fn(&mut shell, "f(){ echo hi; }");
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let args = vec!["-F".to_string(), "f".to_string()];
    run_declaration_builtin_strs("declare", &args, &mut out, &mut err, &mut shell);
    assert_eq!(String::from_utf8(out).unwrap(), "f\n");
}

#[test]
fn declare_dash_f_no_args_keeps_declare_prefix() {
    let mut shell = Shell::new();
    define_fn(&mut shell, "f(){ :; }");
    define_fn(&mut shell, "g(){ :; }");
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let args = vec!["-F".to_string()];
    run_declaration_builtin_strs("declare", &args, &mut out, &mut err, &mut shell);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "declare -f f\ndeclare -f g\n"
    );
}

#[test]
fn declare_big_f_listing_reflects_export_attr_and_filter() {
    let mut shell = Shell::new();
    define_fn(&mut shell, "a(){ :; }");
    define_fn(&mut shell, "zf(){ :; }");
    shell.mark_function_exported("zf");

    // -F listing (want_export=false): plain `declare -f a`, exported `declare -fx zf`.
    let mut out: Vec<u8> = Vec::new();
    declare_list_functions(&[], true, false, &mut out, &mut shell);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "declare -f a\ndeclare -fx zf\n"
    );

    // -xF listing (want_export=true): only the exported function.
    let mut out2: Vec<u8> = Vec::new();
    declare_list_functions(&[], true, true, &mut out2, &mut shell);
    assert_eq!(String::from_utf8(out2).unwrap(), "declare -fx zf\n");
}

#[test]
fn declare_f_named_function_missing() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-F", "fn_none"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn declare_invalid_identifier() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["1foo=bar"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    assert!(shell.lookup_var("1foo").is_none());
}

#[test]
fn declare_typeset_alias() {
    let mut shell = Shell::new();
    let oc = run_typeset(&["-r", "X_TS=hi"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_TS").as_deref(), Some("hi"));
    assert!(shell.is_readonly("X_TS"));
}

#[test]
fn declare_nameref_basic() {
    // `declare -n r=x` binds r as a nameref pointing at x.
    let mut shell = Shell::new();
    let (oc, _) = run(&["-n", "r=x"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.is_nameref("r"));
    // After Task 3: lookup_var dereferences the nameref; x is unset → None.
    assert_eq!(shell.lookup_var("r"), None);
    // The raw target name is still "x".
    assert_eq!(shell.nameref_raw_target("r").as_deref(), Some("x"));
}

#[test]
fn declare_nameref_self_ref_errors() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-n", "r=r"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn declare_nameref_invalid_target_errors() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-n", "r=a b"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn declare_nameref_subscript_target() {
    // `declare -n e=arr[0]` should succeed.
    let mut shell = Shell::new();
    let (oc, _) = run(&["-n", "e=arr[0]"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.is_nameref("e"));
    // After Task 3: lookup_var dereferences the nameref; arr is unset → None.
    assert_eq!(shell.lookup_var("e"), None);
    // The raw target name is still "arr[0]".
    assert_eq!(shell.nameref_raw_target("e").as_deref(), Some("arr[0]"));
}

#[test]
fn declare_plus_n_removes_nameref() {
    // `declare +n r` removes the nameref attribute.
    let mut shell = Shell::new();
    let (oc, _) = run(&["-n", "r=x"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.is_nameref("r"));
    let (oc2, _) = run(&["+n", "r"], &mut shell);
    assert!(matches!(oc2, ExecOutcome::Continue(0)));
    assert!(!shell.is_nameref("r"));
    // Value remains "x" after nameref removal.
    assert_eq!(shell.lookup_var("r").as_deref(), Some("x"));
}

#[test]
fn declare_nameref_bare_unbound() {
    // `declare -n r` (no value) creates an unbound nameref with empty value.
    let mut shell = Shell::new();
    let (oc, _) = run(&["-n", "r"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.is_nameref("r"));
}

#[test]
fn declare_lu_cancel_no_fold() {
    // `declare -lu x=AbC` — both -l and -u cancel to no attribute;
    // the stored value must be unchanged (AbC).
    let mut shell = Shell::new();
    let (oc, _) = run(&["-lu", "X_LU_CANCEL=AbC"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_LU_CANCEL").as_deref(), Some("AbC"));
    assert_eq!(shell.case_fold_of("X_LU_CANCEL"), None);
}

#[test]
fn declare_plus_l_removes_lower_attr() {
    // `declare -l x` then `declare +l x` then assign x=ABC → stored ABC
    // (the lowercase attribute was removed, so no fold occurs).
    let mut shell = Shell::new();
    let (oc, _) = run(&["-l", "X_PL=hello"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_PL").as_deref(), Some("hello"));
    let (oc2, _) = run(&["+l", "X_PL"], &mut shell);
    assert!(matches!(oc2, ExecOutcome::Continue(0)));
    assert_eq!(shell.case_fold_of("X_PL"), None);
    let _ = run(&["X_PL=ABC"], &mut shell);
    assert_eq!(shell.lookup_var("X_PL").as_deref(), Some("ABC"));
}

#[test]
fn declare_plus_u_removes_upper_attr() {
    // `declare -u x` then `declare +u x` then assign x=abc → stored abc.
    let mut shell = Shell::new();
    let (oc, _) = run(&["-u", "X_PU=HELLO"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_PU").as_deref(), Some("HELLO"));
    let (oc2, _) = run(&["+u", "X_PU"], &mut shell);
    assert!(matches!(oc2, ExecOutcome::Continue(0)));
    assert_eq!(shell.case_fold_of("X_PU"), None);
    let _ = run(&["X_PU=abc"], &mut shell);
    assert_eq!(shell.lookup_var("X_PU").as_deref(), Some("abc"));
}

#[test]
fn declare_plus_l_noop_on_upper_attr() {
    // `declare -u x` then `declare +l x` → +l is a no-op (x has Upper,
    // not Lower), so assigning abc still yields ABC.
    let mut shell = Shell::new();
    let (oc, _) = run(&["-u", "X_PL_NOP=hello"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("X_PL_NOP").as_deref(), Some("HELLO"));
    let (oc2, _) = run(&["+l", "X_PL_NOP"], &mut shell);
    assert!(matches!(oc2, ExecOutcome::Continue(0)));
    // Upper attribute must still be present.
    assert_eq!(
        shell.case_fold_of("X_PL_NOP"),
        Some(crate::shell_state::CaseFold::Upper)
    );
    let _ = run(&["X_PL_NOP=abc"], &mut shell);
    assert_eq!(shell.lookup_var("X_PL_NOP").as_deref(), Some("ABC"));
}
