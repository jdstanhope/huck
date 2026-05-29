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
fn readonly_basic_blocks_reassignment() {
    let (out, err) = run_capture(
        "readonly X=1\nX=2\nrc=$?\necho rc=$rc\necho X=$X\nexit\n",
    );
    assert!(
        err.contains("readonly"),
        "expected stderr to mention readonly, got: {err:?}",
    );
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "X=1"), "stdout: {out:?}");
}

#[test]
fn readonly_lists_in_posix_format() {
    let (out, _) = run_capture("readonly X='a b'\nreadonly\nexit\n");
    assert!(
        out.lines().any(|l| l == "readonly X='a b'"),
        "stdout: {out:?}",
    );
}

#[test]
fn readonly_blocks_unset() {
    let (out, err) = run_capture(
        "readonly X=1\nunset X\nrc=$?\necho rc=$rc\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn readonly_blocks_inline_assignment() {
    let (out, err) = run_capture(
        "readonly X=1\nX=2 echo hi\nrc=$?\necho rc=$rc\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
    assert!(
        !out.lines().any(|l| l == "hi"),
        "echo should not have run; stdout: {out:?}",
    );
}

#[test]
fn readonly_blocks_for_loop() {
    let (out, err) = run_capture(
        "readonly X=outer\nfor X in a b c; do echo got=$X; done\nrc=$?\necho rc=$rc\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(
        !out.lines().any(|l| l.starts_with("got=")),
        "loop body should not run; stdout: {out:?}",
    );
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn readonly_with_single_quote_listing_escapes() {
    let (out, _) = run_capture(
        "readonly X=\"a'b\"\nreadonly\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == r"readonly X='a'\''b'"),
        "stdout: {out:?}",
    );
}
