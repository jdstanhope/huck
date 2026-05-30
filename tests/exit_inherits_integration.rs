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
fn bare_exit_after_false_returns_1() {
    let (_out, _err, rc) = run_capture("false\nexit\n");
    assert_eq!(
        rc, 1,
        "expected exit code 1 (inheriting `false`'s status); got {rc}"
    );
}

#[test]
fn bare_exit_after_true_returns_0() {
    let (_out, _err, rc) = run_capture("true\nexit\n");
    assert_eq!(
        rc, 0,
        "expected exit code 0 (inheriting `true`'s status); got {rc}"
    );
}
