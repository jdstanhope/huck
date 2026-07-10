use super::*;
use crate::shell_state::Shell;

fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("type", &args_owned, &mut buf, &mut std::io::stderr(), shell);
    (outcome, String::from_utf8(buf).unwrap())
}

#[test]
fn type_default_builtin() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["echo"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out.trim_end(), "echo is a shell builtin");
}

#[test]
fn type_default_keyword() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["if"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out.trim_end(), "if is a shell keyword");
}

#[test]
fn type_default_function() {
    let mut shell = Shell::new();
    let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        "myfn(){ :; }",
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .unwrap()
    .unwrap();
    let crate::command::Command::FunctionDef { name, body } = seq.first else {
        panic!("expected function def")
    };
    shell.define_function(name, body);
    let (oc, out) = run(&["myfn"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out, "myfn is a function\nmyfn () \n{ \n    :\n}\n");
}

#[test]
fn type_prints_function_body() {
    let mut shell = Shell::new();
    let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        "tf(){ echo a; }",
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .unwrap()
    .unwrap();
    let crate::command::Command::FunctionDef { name, body } = seq.first else {
        panic!("expected function def")
    };
    shell.define_function(name, body);
    let (oc, out) = run(&["tf"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out, "tf is a function\ntf () \n{ \n    echo a\n}\n");
}

#[test]
fn type_default_alias() {
    let mut shell = Shell::new();
    shell.aliases.insert("ll".to_string(), "ls -l".to_string());
    let (oc, out) = run(&["ll"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out.trim_end(), "ll is aliased to `ls -l'");
}

#[test]
fn type_default_not_found() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["__xyz_no_such_cmd__"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    assert!(out.is_empty(), "stdout should be empty, got: {out:?}");
}

#[test]
fn type_t_builtin() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["-t", "echo"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out.trim_end(), "builtin");
}

#[test]
fn type_t_keyword() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["-t", "if"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out.trim_end(), "keyword");
}

#[test]
fn type_t_function() {
    let mut shell = Shell::new();
    let body = Box::new(crate::command::Command::Simple(
        crate::command::SimpleCommand::Assign(vec![], 0),
    ));
    shell.define_function("myfn".to_string(), body);
    let (oc, out) = run(&["-t", "myfn"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(out.trim_end(), "function");
}

#[test]
fn type_t_not_found_silent() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["-t", "__xyz_no_such_cmd__"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    assert!(out.is_empty(), "stdout should be empty, got: {out:?}");
}

#[test]
fn type_p_builtin_silent() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["-p", "echo"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(out.is_empty(), "stdout should be empty, got: {out:?}");
}

#[test]
fn type_a_alias_and_builtin() {
    // alias "echo=foo" + builtin "echo": -a should list both.
    let mut shell = Shell::new();
    shell.aliases.insert("echo".to_string(), "foo".to_string());
    let (oc, out) = run(&["-a", "echo"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines.iter().any(|l| l.contains("aliased to `foo'")),
        "expected alias line; got: {lines:?}",
    );
    assert!(
        lines.contains(&"echo is a shell builtin"),
        "expected builtin line; got: {lines:?}",
    );
}

#[test]
fn type_f_skips_function() {
    let mut shell = Shell::new();
    let body = Box::new(crate::command::Command::Simple(
        crate::command::SimpleCommand::Assign(vec![], 0),
    ));
    shell.define_function("myfn".to_string(), body);
    // Without -f: would find the function.
    let (oc, _) = run(&["-f", "myfn"], &mut shell);
    // With -f: function ignored, no other match → not found.
    assert!(matches!(oc, ExecOutcome::Continue(1)));
}

#[test]
fn type_capital_p_force_path() {
    // type -P sh: skip builtin precedence, look up sh in PATH.
    // Test environment is expected to have /bin/sh or /usr/bin/sh.
    let mut shell = Shell::new();
    let (oc, out) = run(&["-P", "sh"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(
        out.lines().any(|l| l.ends_with("/sh")),
        "expected a path ending in /sh; got: {out:?}",
    );
}

#[test]
fn type_multi_name_first_found_second_missing() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["echo", "__xyz_no_such_cmd__"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(1)));
    assert!(
        out.lines().any(|l| l == "echo is a shell builtin"),
        "stdout should have echo line; got: {out:?}",
    );
}

#[test]
fn type_invalid_option_status_2() {
    let mut shell = Shell::new();
    let (oc, _out) = run(&["-X", "echo"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(2)));
}
