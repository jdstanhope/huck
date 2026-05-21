use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> String {
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn test_f_on_existing_file_is_true() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("afile");
    std::fs::write(&file, b"x").unwrap();
    let script = format!("test -f '{}'\necho $?\nexit\n", file.to_str().unwrap());
    let out = run(&script);
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn test_f_on_missing_file_is_false() {
    let out = run("test -f /no/such/huck/path\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
}

#[test]
fn bracket_d_on_directory_is_true() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!("[ -d '{}' ]\necho $?\nexit\n", dir.path().to_str().unwrap());
    let out = run(&script);
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn bracket_string_equality() {
    let out = run("[ abc = abc ]\necho $?\n[ abc = xyz ]\necho $?\nexit\n");
    let codes: Vec<&str> = out.lines().filter(|l| *l == "0" || *l == "1").collect();
    assert_eq!(codes, vec!["0", "1"], "stdout: {out}");
}

#[test]
fn bracket_integer_comparison() {
    let out = run("[ 3 -lt 10 ]\necho $?\n[ 10 -lt 3 ]\necho $?\nexit\n");
    let codes: Vec<&str> = out.lines().filter(|l| *l == "0" || *l == "1").collect();
    assert_eq!(codes, vec!["0", "1"], "stdout: {out}");
}

#[test]
fn bracket_negation() {
    let out = run("[ ! -f /no/such/huck/path ]\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn bracket_missing_close_sets_status_two() {
    let out = run("[ -f foo\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "2"), "stdout: {out}");
}

#[test]
fn test_non_integer_operand_sets_status_two() {
    let out = run("test abc -eq 1\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "2"), "stdout: {out}");
}

#[test]
fn bracket_with_expanded_variable() {
    // `[ "$x" = foo ]` with x unset -> "" = foo -> false (status 1).
    let out = run("[ \"$x\" = foo ]\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
}
