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
fn help_lists_known_builtin() {
    let (out, _, _) = run_capture("help\nexit\n");
    assert!(out.lines().any(|l| l.starts_with("cd:")), "stdout: {out:?}");
    assert!(
        out.lines().any(|l| l.starts_with("echo:")),
        "stdout: {out:?}"
    );
}

#[test]
fn help_named_includes_synopsis_and_description() {
    let (out, _, _) = run_capture("help cd\nexit\n");
    assert!(out.lines().any(|l| l.starts_with("cd:")), "stdout: {out:?}");
    assert!(
        out.lines().any(|l| l.starts_with("    ")),
        "expected an indented description line; stdout: {out:?}",
    );
}

#[test]
fn help_s_synopsis_only() {
    let (out, _, _) = run_capture("help -s echo\nexit\n");
    let echo_lines: Vec<&str> = out.lines().filter(|l| l.starts_with("echo:")).collect();
    assert_eq!(echo_lines.len(), 1, "stdout: {out:?}");
}

#[test]
fn help_unknown_errors() {
    let (out, err, _) = run_capture("help __no_such_builtin__\nrc=$?\necho rc=$rc\nexit\n");
    assert!(err.contains("no help topics match"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn help_man_format_has_sections() {
    let (out, _, _) = run_capture("help -m cd\nexit\n");
    assert!(out.lines().any(|l| l == "NAME"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "SYNOPSIS"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "DESCRIPTION"), "stdout: {out:?}");
}
