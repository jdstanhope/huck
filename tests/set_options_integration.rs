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
fn set_e_exits_on_failure() {
    let (out, _, rc) = run_capture("set -e\nfalse\necho X\nexit\n");
    assert_eq!(rc, 1, "expected rc=1; got {rc}; stdout: {out:?}");
    assert!(
        !out.lines().any(|l| l == "X"),
        "stdout should not have X: {out:?}"
    );
}

#[test]
fn set_e_exempt_in_if() {
    let (out, _, _) = run_capture("set -e\nif false; then :; fi\necho X\nexit\n");
    assert!(
        out.lines().any(|l| l == "X"),
        "expected X in stdout: {out:?}"
    );
}

#[test]
fn set_e_exempt_in_or_chain() {
    let (out, _, _) = run_capture("set -e\nfalse || true\necho X\nexit\n");
    assert!(
        out.lines().any(|l| l == "X"),
        "expected X in stdout: {out:?}"
    );
}

#[test]
fn set_e_in_function_exits() {
    let (out, _, rc) = run_capture("set -e\nf() { false; echo F; }\nf\necho M\nexit\n");
    assert_eq!(rc, 1, "expected rc=1; got {rc}; stdout: {out:?}");
    assert!(!out.lines().any(|l| l == "F"));
    assert!(!out.lines().any(|l| l == "M"));
}

#[test]
fn set_u_unset_errors() {
    let (out, err, rc) = run_capture("set -u\necho $XYZ_UNSET\necho X\nexit\n");
    // In non-interactive mode, fatal PE error exits the shell.
    assert!(err.contains("unbound variable"), "stderr: {err:?}");
    assert!(!out.lines().any(|l| l == "X"), "stdout: {out:?}");
    assert_ne!(rc, 0, "expected non-zero rc; got {rc}");
}

#[test]
fn set_u_default_modifier_ok() {
    let (out, _, _) = run_capture("set -u\necho \"${XYZ_UNSET:-default}\"\necho X\nexit\n");
    assert!(out.lines().any(|l| l == "default"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "X"), "stdout: {out:?}");
}

#[test]
fn set_o_errexit_works_as_dash_e() {
    let (out, _, rc) = run_capture("set -o errexit\nfalse\necho X\nexit\n");
    assert_eq!(rc, 1, "expected rc=1; got {rc}");
    assert!(!out.lines().any(|l| l == "X"));
}

#[test]
fn dollar_dash_includes_e_after_set_e() {
    let (out, _, _) = run_capture("set -e\necho \"[$-]\"\nexit\n");
    let line = out
        .lines()
        .find(|l| l.starts_with("[") && l.ends_with("]"))
        .unwrap_or("");
    assert!(line.contains('e'), "$- should contain 'e'; got: {line:?}");
}

#[test]
fn set_minus_o_lists_options() {
    let (out, _, _) = run_capture("set -o\nexit\n");
    assert!(
        out.lines().any(|l| l.starts_with("errexit")),
        "stdout: {out:?}"
    );
    assert!(
        out.lines().any(|l| l.starts_with("nounset")),
        "stdout: {out:?}"
    );
}
