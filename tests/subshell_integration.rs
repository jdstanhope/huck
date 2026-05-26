//! End-to-end tests for v28 subshell syntax.

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
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn subshell_basic_echo() {
    let (out, _) = run("(echo hi)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn subshell_isolates_var_assignment() {
    let (out, _) = run("FOO=outer\n(FOO=inner; echo in:$FOO)\necho out:$FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "in:inner"), "got: {out}");
    assert!(out.lines().any(|l| l.trim() == "out:outer"), "got: {out}");
}

#[test]
fn subshell_isolates_cd() {
    // Capture cwd before and after a subshell `cd`; should be identical.
    let (out, _) = run("pwd > /tmp/v28_cd_before_$$\n(cd /tmp)\npwd > /tmp/v28_cd_after_$$\ndiff /tmp/v28_cd_before_$$ /tmp/v28_cd_after_$$ && echo SAME\nrm -f /tmp/v28_cd_before_$$ /tmp/v28_cd_after_$$\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "SAME"), "got: {out}");
}

#[test]
fn subshell_isolates_function_def() {
    // Function defined inside subshell is gone after subshell exits.
    let (out, err) = run("(f() { echo defined; }; f)\nf\necho post-status=$?\nexit\n");
    // Inside subshell: `defined` line printed.
    assert!(out.lines().any(|l| l.trim() == "defined"), "got out: {out} err: {err}");
    // Outside: f is not defined; either stderr says "not found" or post-status != 0.
    let combined = format!("{out}{err}");
    assert!(combined.contains("not found") || out.contains("post-status=127") || out.contains("post-status=1"),
        "expected function-not-found indicator, got out: {out} err: {err}");
}

#[test]
fn subshell_exit_status_propagates() {
    let (out, _) = run("(exit 7)\necho $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "7"), "got: {out}");
}

#[test]
fn subshell_with_sequence() {
    let (out, _) = run("(echo a; echo b)\nexit\n");
    let lines: Vec<&str> = out.lines().filter(|l| l.trim() == "a" || l.trim() == "b").collect();
    assert_eq!(lines, vec!["a", "b"], "got: {out}");
}

#[test]
fn subshell_with_and_or() {
    let (out, _) = run("(true && echo ok)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn subshell_in_pipeline_first_stage() {
    let (out, _) = run("(echo hi) | cat\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn subshell_in_pipeline_last_stage() {
    let (out, _) = run("echo hi | (cat)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn subshell_nested_double_fork() {
    let (out, _) = run("((echo nested))\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "nested"), "got: {out}");
}

#[test]
fn subshell_backgrounded() {
    let tmp = format!("/tmp/v28_bg_{}", std::process::id());
    let script = format!(
        "(echo bg > {tmp}) &\nwait\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "bg"), "got: {out}");
}

#[test]
fn subshell_inherits_vars_from_parent() {
    let (out, _) = run("FOO=hi\n(echo got:$FOO)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "got:hi"), "got: {out}");
}

#[test]
fn subshell_in_function_body() {
    let (out, _) = run("f() { (echo from-subshell-in-func); }\nf\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "from-subshell-in-func"), "got: {out}");
}

#[test]
fn subshell_with_heredoc_inside() {
    let (out, _) = run("(cat <<EOF\nbody\nEOF\n)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "body"), "got: {out}");
}

#[test]
fn subshell_with_here_string_inside() {
    let (out, _) = run("(cat <<< hi)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}
