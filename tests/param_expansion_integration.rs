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
fn use_default_unset_uses_default() {
    let (out, _) = run("echo ${X:-default}\nexit\n");
    assert!(out.lines().any(|l| l == "default"), "stdout: {out}");
}

#[test]
fn use_default_set_uses_value() {
    let (out, _) = run("X=value\necho ${X:-default}\nexit\n");
    assert!(out.lines().any(|l| l == "value"), "stdout: {out}");
}

#[test]
fn assign_default_mutates_shell() {
    let (out, _) = run("echo ${X:=default}\necho $X\nexit\n");
    let lines: Vec<&str> = out.lines().filter(|l| *l == "default").collect();
    assert!(lines.len() >= 2, "expected 'default' twice, stdout: {out}");
}

#[test]
fn assign_default_no_colon_mutates_when_unset() {
    let (out, _) = run("echo ${X=assigned}\necho $X\nexit\n");
    let lines: Vec<&str> = out.lines().filter(|l| *l == "assigned").collect();
    assert!(lines.len() >= 2, "expected 'assigned' twice, stdout: {out}");
}

#[test]
fn error_if_unset_writes_to_stderr() {
    let (_, err) = run("echo ${UNSET:?missing}\nexit\n");
    assert!(err.contains("UNSET: missing"), "stderr: {err}");
}

#[test]
fn use_alternate_set_uses_alternate() {
    let (out, _) = run("X=anything\necho ${X:+set}\nexit\n");
    assert!(out.lines().any(|l| l == "set"), "stdout: {out}");
}

#[test]
fn use_alternate_unset_yields_empty() {
    let (out, _) = run("echo ${X:+set}\nexit\n");
    assert!(out.lines().any(|l| l.is_empty()), "stdout: {out}");
}

#[test]
fn remove_prefix_longest_strips_path() {
    let (out, _) = run("f=/path/to/file.txt\necho ${f##*/}\nexit\n");
    assert!(out.lines().any(|l| l == "file.txt"), "stdout: {out}");
}

#[test]
fn remove_suffix_strips_extension() {
    let (out, _) = run("f=/path/to/file.txt\necho ${f%.*}\nexit\n");
    assert!(out.lines().any(|l| l == "/path/to/file"), "stdout: {out}");
}

#[test]
fn length_of_set_string() {
    let (out, _) = run("s=hello\necho ${#s}\nexit\n");
    assert!(out.lines().any(|l| l == "5"), "stdout: {out}");
}

#[test]
fn nested_command_sub_in_default() {
    let (out, _) = run("echo \"${X:-$(echo nested)}\"\nexit\n");
    assert!(out.lines().any(|l| l == "nested"), "stdout: {out}");
}

#[test]
fn quoted_default_with_spaces_stays_one_arg() {
    let (out, _) = run("echo \"${X:-a b c}\"\nexit\n");
    assert!(out.lines().any(|l| l == "a b c"), "stdout: {out}");
}
