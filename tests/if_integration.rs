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
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn if_then_taken_for_true_condition() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("present");
    std::fs::write(&file, b"x").unwrap();
    let script = format!(
        "if test -f '{}'; then echo yes; else echo no; fi\nexit\n",
        file.to_str().unwrap()
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l == "yes"), "stdout: {out}");
}

#[test]
fn if_else_taken_for_false_condition() {
    let (out, _) = run("if test -f /no/such/huck/path; then echo yes; else echo no; fi\nexit\n");
    assert!(out.lines().any(|l| l == "no"), "stdout: {out}");
}

#[test]
fn elif_chain_selects_middle_branch() {
    let (out, _) = run(
        "if test 1 -eq 2; then echo a; elif test 2 -eq 2; then echo b; else echo c; fi\nexit\n",
    );
    assert!(out.lines().any(|l| l == "b"), "stdout: {out}");
}

#[test]
fn if_multi_command_body() {
    let (out, _) = run("if test 1 -eq 1; then echo one; echo two; fi\nexit\n");
    assert!(out.lines().any(|l| l == "one"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "two"), "stdout: {out}");
}

#[test]
fn if_chained_with_and() {
    let (out, _) = run("if test 1 -eq 1; then echo body; fi && echo chained\nexit\n");
    assert!(out.lines().any(|l| l == "body"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "chained"), "stdout: {out}");
}

#[test]
fn nested_if() {
    let (out, _) = run(
        "if test 1 -eq 1; then if test 2 -eq 2; then echo deep; fi; fi\nexit\n",
    );
    assert!(out.lines().any(|l| l == "deep"), "stdout: {out}");
}

#[test]
fn if_status_reflects_branch() {
    let (out, _) = run("if test 1 -eq 1; then test 1 -eq 2; fi\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
}

#[test]
fn if_no_else_no_match_status_zero() {
    let (out, _) = run("if test 1 -eq 2; then echo a; fi\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn unterminated_if_is_syntax_error() {
    let (_, err) = run("if test 1 -eq 1; then echo x\nexit\n");
    assert!(err.to_lowercase().contains("syntax error"), "stderr: {err}");
}

#[test]
fn echo_if_prints_if() {
    let (out, _) = run("echo if\nexit\n");
    assert!(out.lines().any(|l| l == "if"), "stdout: {out}");
}
