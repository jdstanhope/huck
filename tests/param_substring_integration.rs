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
fn substring_basic_offset_only() {
    let (out, _) = run("s=hello\necho ${s:1}\nexit\n");
    assert!(out.lines().any(|l| l == "ello"), "stdout: {out}");
}

#[test]
fn substring_offset_and_length() {
    let (out, _) = run("s=hello\necho ${s:1:3}\nexit\n");
    assert!(out.lines().any(|l| l == "ell"), "stdout: {out}");
}

#[test]
fn substring_offset_equals_strlen_is_empty() {
    let (out, _) = run("s=abc\necho \"[${s:3}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn substring_offset_beyond_strlen_is_empty() {
    let (out, _) = run("s=abc\necho \"[${s:5}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn substring_negative_offset_with_space() {
    // The space disambiguates from :- (UseDefault).
    let (out, _) = run("s=hello\necho ${s: -2}\nexit\n");
    assert!(out.lines().any(|l| l == "lo"), "stdout: {out}");
}

#[test]
fn substring_no_space_remains_use_default_regression() {
    // ${s:-default} must still mean UseDefault, not substring with offset=-default.
    let (out, _) = run("unset MAYBE 2>/dev/null\necho ${MAYBE:-fallback}\nexit\n");
    assert!(out.lines().any(|l| l == "fallback"), "stdout: {out}");
}

#[test]
fn substring_negative_length_counts_from_end() {
    // eff_len = 5 + -1 - 1 = 3.
    let (out, _) = run("s=hello\necho ${s:1:-1}\nexit\n");
    assert!(out.lines().any(|l| l == "ell"), "stdout: {out}");
}

#[test]
fn substring_negative_computed_length_errors() {
    let (out, err) = run("s=abc\necho \"[${s:0:-4}]\"\nexit\n");
    // The error path returns Empty (so the field is empty) and sets $? to 1.
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(err.contains("substring expression < 0"), "stderr: {err}");
}

#[test]
fn substring_unset_var_is_empty() {
    let (out, _) = run("echo \"[${MISSING:0:3}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn substring_var_ref_in_offset() {
    let (out, _) = run("s=hello\nn=2\necho ${s:$n}\nexit\n");
    assert!(out.lines().any(|l| l == "llo"), "stdout: {out}");
}

#[test]
fn substring_arith_in_length() {
    let (out, _) = run("s=hello\nn=1\necho ${s:1:$((n+1))}\nexit\n");
    assert!(out.lines().any(|l| l == "el"), "stdout: {out}");
}

#[test]
fn substring_unicode() {
    let (out, _) = run("s=café\necho ${s:1:2}\nexit\n");
    assert!(out.lines().any(|l| l == "af"), "stdout: {out}");
}

#[test]
fn substring_inside_quotes_single_field() {
    // "${s:1:3}" with internal whitespace stays as one field (no IFS-split).
    let (out, _) = run("s=\"hi world\"\necho \"[${s:1:5}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[i wor]"), "stdout: {out}");
}

#[test]
fn substring_in_pipeline_stage() {
    let (out, _) = run("s=hello\necho ${s:1:3} | cat\nexit\n");
    assert!(out.lines().any(|l| l == "ell"), "stdout: {out}");
}

#[test]
fn substring_positional_in_function() {
    let (out, _) = run("f() { echo \"${1:0:3}\"; }\nf hello\nexit\n");
    assert!(out.lines().any(|l| l == "hel"), "stdout: {out}");
}

#[test]
fn substring_bad_arith_returns_empty_sets_status() {
    let (out, err) = run("s=hello\necho \"[${s:@@@}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(err.contains("arithmetic"), "stderr: {err}");
}
