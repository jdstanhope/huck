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
fn arith_basic_precedence() {
    let (out, _) = run("echo $((2+3*4))\nexit\n");
    assert!(out.lines().any(|l| l == "14"), "stdout: {out}");
}

#[test]
fn arith_with_variable() {
    let (out, _) = run("x=5\necho $((x*2))\nexit\n");
    assert!(out.lines().any(|l| l == "10"), "stdout: {out}");
}

#[test]
fn arith_assignment_rhs() {
    let (out, _) = run("FOO=$((1+1))\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l == "2"), "stdout: {out}");
}

#[test]
fn arith_negative_result() {
    let (out, _) = run("echo $((-5*3))\nexit\n");
    assert!(out.lines().any(|l| l == "-15"), "stdout: {out}");
}

#[test]
fn arith_inside_double_quotes() {
    let (out, _) = run("echo \"answer: $((6*7))\"\nexit\n");
    assert!(out.lines().any(|l| l == "answer: 42"), "stdout: {out}");
}

#[test]
fn arith_division_by_zero_writes_to_stderr() {
    let (_, err) = run("echo $((1/0))\nexit\n");
    assert!(err.contains("division by zero"), "stderr: {err}");
}

#[test]
fn arith_ternary() {
    let (out, _) = run("echo $((1<2 ? 100 : 200))\nexit\n");
    assert!(out.lines().any(|l| l == "100"), "stdout: {out}");
}

#[test]
fn arith_logical_short_circuit() {
    // The RHS `1/0` would error if evaluated; short-circuit must prevent that.
    let (out, err) = run("echo $((0 && 1/0))\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
    assert!(!err.contains("division by zero"), "stderr: {err}");
}
