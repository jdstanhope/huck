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
fn case_upper_all_basic() {
    let (out, _) = run("s=hello\necho ${s^^}\nexit\n");
    assert!(out.lines().any(|l| l == "HELLO"), "stdout: {out}");
}

#[test]
fn case_upper_first_basic() {
    let (out, _) = run("s=hello\necho ${s^}\nexit\n");
    assert!(out.lines().any(|l| l == "Hello"), "stdout: {out}");
}

#[test]
fn case_lower_all_basic() {
    let (out, _) = run("s=HELLO\necho ${s,,}\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out}");
}

#[test]
fn case_lower_first_basic() {
    let (out, _) = run("s=HELLO\necho ${s,}\nexit\n");
    assert!(out.lines().any(|l| l == "hELLO"), "stdout: {out}");
}

#[test]
fn case_upper_with_pattern_filters() {
    // Only vowels get upper-cased.
    let (out, _) = run("s=hello\necho ${s^^[aeiou]}\nexit\n");
    assert!(out.lines().any(|l| l == "hEllO"), "stdout: {out}");
}

#[test]
fn case_upper_unicode() {
    // Unicode-aware: é → É.
    let (out, _) = run("s=café\necho ${s^^}\nexit\n");
    assert!(out.lines().any(|l| l == "CAFÉ"), "stdout: {out}");
}

#[test]
fn case_pattern_uses_other_var() {
    // The pattern is expanded — $p resolves to [ae] before glob compile.
    let (out, _) = run("s=hello\np=[ae]\necho ${s^^$p}\nexit\n");
    assert!(out.lines().any(|l| l == "hEllo"), "stdout: {out}");
}

#[test]
fn case_in_function_with_positional() {
    let (out, _) = run("f() { echo \"${1^^}\"; }\nf hello\nexit\n");
    assert!(out.lines().any(|l| l == "HELLO"), "stdout: {out}");
}

#[test]
fn case_in_pipeline_stage() {
    let (out, _) = run("s=hello\necho ${s^^} | cat\nexit\n");
    assert!(out.lines().any(|l| l == "HELLO"), "stdout: {out}");
}
