use super::*;
use crate::shell_state::Shell;

#[test]
fn command_no_args_exits_zero() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("command", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn command_builtin_bare_form_still_errors_when_called_directly() {
    // As of v99 the bare form `command CMD args` is handled in the executor
    // (`run_exec_single` rewrites the program and bypasses function lookup
    // before the `command` builtin is ever reached). The builtin itself
    // retains its defensive rejection for the bare form when invoked
    // directly (e.g. via run_builtin), which this test asserts.
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["echo".to_string(), "hi".to_string()];
    let outcome = run_builtin(
        "command",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn command_dash_v_builtin_concise() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["-v".to_string(), "echo".to_string()];
    let outcome = run_builtin(
        "command",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out.trim_end(), "echo");
}

#[test]
fn command_dash_v_notfound_silent_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["-v".to_string(), "__no_such_cmd_xyzzy__".to_string()];
    let outcome = run_builtin(
        "command",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    let out = String::from_utf8(buf).unwrap();
    assert!(out.is_empty(), "expected silent stdout, got: {out:?}");
}

#[test]
fn command_dash_v_builtin_verbose() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["-V".to_string(), "echo".to_string()];
    let outcome = run_builtin(
        "command",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out.trim_end(), "echo is a shell builtin");
}

#[test]
fn command_dash_v_keyword_verbose() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["-V".to_string(), "if".to_string()];
    let outcome = run_builtin(
        "command",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out.trim_end(), "if is a shell keyword");
}

#[test]
fn command_dash_v_function() {
    let mut shell = Shell::new();
    // Register a function directly. The body shape is irrelevant for
    // resolution; any Command value works. Use a no-op assignment list.
    let body = Box::new(crate::command::Command::Simple(
        crate::command::SimpleCommand::Assign(vec![], 0),
    ));
    shell.define_function("myfn".to_string(), body, 0);
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["-v".to_string(), "myfn".to_string()];
    let outcome = run_builtin(
        "command",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out.trim_end(), "myfn");
}

#[test]
fn command_dash_v_alias_with_single_quote_escapes() {
    let mut shell = Shell::new();
    shell
        .aliases
        .insert("greet".to_string(), "echo it's me".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let args = vec!["-v".to_string(), "greet".to_string()];
    let outcome = run_builtin(
        "command",
        &args,
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out.trim_end(), r"alias greet='echo it'\''s me'");
}
