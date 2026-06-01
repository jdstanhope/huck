use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn eval_simple_command() {
    let (out, _, _) = run_capture("eval echo hi\nexit\n");
    assert!(out.lines().any(|l| l == "hi"), "stdout: {out:?}");
}

#[test]
fn eval_multi_statement() {
    let (out, _, _) = run_capture("eval 'echo a; echo b'\nexit\n");
    let collected: Vec<&str> = out.lines().take(2).collect();
    assert_eq!(collected, vec!["a", "b"], "stdout: {out:?}");
}

#[test]
fn eval_assignment_persists() {
    let (out, _, _) = run_capture("eval 'X=hello'\necho \"[$X]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[hello]"), "stdout: {out:?}");
}

#[test]
fn eval_exit_propagates() {
    let (out, _, rc) = run_capture("eval 'exit 7'\necho unreached\nexit\n");
    assert_eq!(rc, 7, "expected exit 7; got {rc}; stdout: {out:?}");
    assert!(
        !out.lines().any(|l| l == "unreached"),
        "stdout should not contain `unreached`; got: {out:?}",
    );
}
