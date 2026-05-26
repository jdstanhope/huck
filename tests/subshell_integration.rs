//! End-to-end tests for v28 subshell syntax.

use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn subshell_basic_echo() {
    let (out, _) = run("(echo hi)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn subshell_isolates_var_assignment() {
    let (out, _) = run("FOO=outer\n(FOO=inner; echo in:$FOO)\necho out:$FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "in:inner"), "got: {out}");
    assert!(out.lines().any(|l| l.trim() == "out:outer"), "got: {out}");
}
