use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn colon_is_no_op() {
    let (out, _) = run_capture(": anything\necho ok\nexit\n");
    assert!(out.lines().any(|l| l == "ok"), "stdout: {out:?}");
}

#[test]
fn colon_triggers_param_default_assignment() {
    let (out, _) = run_capture(": ${X:=hello}\necho \"$X\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out:?}");
}

#[test]
fn true_in_conditional() {
    let (out, _) = run_capture("if true; then echo Y; fi\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "stdout: {out:?}");
}

#[test]
fn false_in_conditional() {
    let (out, _) = run_capture("if false; then echo Y; else echo N; fi\nexit\n");
    assert!(out.lines().any(|l| l == "N"), "stdout: {out:?}");
}

#[test]
fn command_v_finds_builtin() {
    let (out, _) = run_capture("command -v echo\nexit\n");
    assert!(
        out.lines().any(|l| l == "echo"),
        "expected `echo` line, got: {out:?}"
    );
}

#[test]
fn command_v_missing_status_1() {
    let (out, _) = run_capture("command -v __no_such_cmd_xyzzy__\nrc=$?\necho rc=$rc\nexit\n");
    let rc_line = out
        .lines()
        .find(|l| l.starts_with("rc="))
        .unwrap_or_else(|| panic!("no rc= line; got: {out:?}"));
    assert_eq!(rc_line, "rc=1", "stdout: {out:?}");
}

#[test]
fn command_v_finds_path_binary() {
    let (out, _) = run_capture("command -v sh\nexit\n");
    let sh_line = out.lines().find(|l| l.contains('/'));
    assert!(
        sh_line.is_some(),
        "expected a path containing `/`, got: {out:?}"
    );
}

#[test]
fn command_uppercase_v_keyword() {
    let (out, _) = run_capture("command -V if\nexit\n");
    assert!(
        out.lines().any(|l| l == "if is a shell keyword"),
        "stdout: {out:?}"
    );
}
