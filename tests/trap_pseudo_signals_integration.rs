use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String, std::process::ExitStatus) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status,
    )
}

// ──────────── DEBUG (4 tests) ────────────

#[test]
fn debug_fires_before_simple_command() {
    let (out, _err, _) = run("trap 'echo DBG' DEBUG\ntrue\nexit\n");
    assert!(out.lines().any(|l| l == "DBG"), "stdout: {out}");
}

#[test]
fn debug_fires_inside_function_body() {
    let (out, _err, _) = run("trap 'echo DBG' DEBUG\nf() { true; }\nf\nexit\n");
    // At least one DBG fires for the `true` inside f.
    let count = out.lines().filter(|l| **l == *"DBG").count();
    assert!(count >= 1, "expected ≥1 DBG, got {count}; stdout: {out}");
}

#[test]
fn debug_does_not_fire_for_compound_command_itself() {
    // `if true; then true; fi` has TWO simple commands (condition's
    // `true` + body's `true`), plus the trailing `exit` = 3 total.
    // DEBUG fires for simple commands only — not for the `if` compound
    // itself. The action's own `echo DBG` is recursion-suppressed.
    let (out, _err, _) = run("trap 'echo DBG' DEBUG\nif true; then true; fi\nexit\n");
    let count = out.lines().filter(|l| **l == *"DBG").count();
    // Exactly 3 DBG lines: condition `true`, body `true`, `exit`.
    // No extra lines from the DEBUG action itself (recursion-guarded).
    assert_eq!(count, 3, "expected exactly 3 DBG lines, got {count}; stdout: {out}");
}

#[test]
fn debug_recursion_guard_prevents_infinite_fire() {
    // The trap action itself runs `echo DBG`, which IS a simple command,
    // but the recursion guard suppresses DEBUG re-firing inside the
    // action. So `true` + `exit` produce exactly TWO DBG lines (one per
    // simple command), not an infinite loop.
    let (out, _err, status) = run("trap 'echo DBG' DEBUG\ntrue\nexit\n");
    let count = out.lines().filter(|l| **l == *"DBG").count();
    // 2 DBG lines: one for `true`, one for `exit` — the action's own
    // `echo DBG` is recursion-suppressed, so no runaway firing.
    assert_eq!(count, 2, "expected exactly 2 DBG, got {count}; stdout: {out}");
    assert_eq!(status.code(), Some(0));
}

// ──────────── ERR (7 tests) ────────────

#[test]
fn err_fires_on_simple_command_failure() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nfalse\nexit\n");
    assert!(out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_does_not_fire_in_if_condition() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nif false; then :; fi\nexit\n");
    assert!(!out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_does_not_fire_in_while_condition() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nwhile false; do :; done\nexit\n");
    assert!(!out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_does_not_fire_on_or_chain_lhs() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nfalse || true\nexit\n");
    assert!(!out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_fires_on_or_chain_when_all_fail() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nfalse || false\nexit\n");
    assert!(out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_fires_on_and_chain_lhs_failure() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\nfalse && true\nexit\n");
    assert!(out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

#[test]
fn err_fires_on_and_chain_rhs_failure() {
    let (out, _err, _) = run("trap 'echo CAUGHT' ERR\ntrue && false\nexit\n");
    assert!(out.lines().any(|l| l == "CAUGHT"), "stdout: {out}");
}

// ──────────── RETURN (2 tests) ────────────

#[test]
fn return_fires_after_function_return() {
    let (out, _err, _) = run("trap 'echo RET' RETURN\nf() { :; }\nf\nexit\n");
    let count = out.lines().filter(|l| **l == *"RET").count();
    assert_eq!(count, 1, "expected exactly 1 RET, got {count}; stdout: {out}");
}

#[test]
fn return_action_sees_function_status() {
    // The action runs with $? set to the function's return status.
    let (out, _err, _) = run("trap 'echo got=$?' RETURN\nf() { return 7; }\nf\necho done=$?\nexit\n");
    assert!(out.lines().any(|l| l == "got=7"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "done=7"), "stdout: {out}");
}
