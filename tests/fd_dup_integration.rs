//! End-to-end tests for v29 fd-duplication redirects.

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
fn dup_stderr_to_stdout_canonical() {
    // sh -c 'echo stderr-msg >&2' writes to stderr. With 2>&1 from huck,
    // the message should appear on huck's stdout (our run() captures it).
    let (out, _err) = run("sh -c 'echo stderr-msg >&2' 2>&1\nexit\n");
    assert!(out.contains("stderr-msg"), "got stdout: {out}");
}
