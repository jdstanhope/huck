//! End-to-end tests for v29 fd-duplication redirects.

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
fn dup_stderr_to_stdout_canonical() {
    // sh -c 'echo stderr-msg >&2' writes to stderr. With 2>&1 from huck,
    // the message should appear on huck's stdout (our run() captures it).
    let (out, _err) = run("sh -c 'echo stderr-msg >&2' 2>&1\nexit\n");
    assert!(out.contains("stderr-msg"), "got stdout: {out}");
}

#[test]
fn dup_stdout_to_stderr() {
    let tmp = format!("/tmp/v29_dup_stdout_{}", std::process::id());
    let script = format!(
        "echo hi 1>&2 2> {tmp}\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn combined_redirect_canonical_form() {
    let tmp = format!("/tmp/v29_combined_{}", std::process::id());
    let script = format!(
        "sh -c 'echo out; echo err >&2' >{tmp} 2>&1\nwc -l < {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "2"), "got: {out}");
}

#[test]
fn and_redir_out_to_file() {
    let tmp = format!("/tmp/v29_andout_{}", std::process::id());
    let script = format!(
        "sh -c 'echo out; echo err >&2' &>{tmp}\nwc -l < {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "2"), "got: {out}");
}

#[test]
fn and_redir_append_to_file() {
    let tmp = format!("/tmp/v29_andappend_{}", std::process::id());
    let script = format!(
        "echo first > {tmp}\nsh -c 'echo second; echo err >&2' &>>{tmp}\nwc -l < {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "3"), "got: {out}");
}

#[test]
fn dup_in_pipeline_stage() {
    let (out, _) = run("sh -c 'echo a; echo b >&2' 2>&1 | grep -c .\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "2"), "got: {out}");
}

#[test]
fn dup_with_inline_assignment() {
    let (out, _) = run("FOO=hi sh -c 'echo $FOO >&2' 2>&1\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn dup_with_subshell_inner_form() {
    // Outer form `(cmd) 2>&1` requires compound-command redirects (separate gap).
    // Inner form `(cmd 2>&1)` works via existing subshell + dup composition.
    let (out, _) = run("(sh -c 'echo from-sub >&2' 2>&1)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "from-sub"), "got: {out}");
}

#[test]
fn dup_runtime_bad_fd_target() {
    // Non-numeric target → runtime error.
    let (out, err) = run("sh -c true 2>&notanumber\nexit\n");
    let combined = format!("{out}{err}");
    assert!(combined.contains("bad fd") || combined.contains("notanumber"),
        "expected bad-fd error, got out: {out} err: {err}");
}

#[test]
fn echo_to_stderr_shorthand() {
    let tmp = format!("/tmp/v29_shorthand_{}", std::process::id());
    let script = format!(
        "echo error >&2 2> {tmp}\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "error"), "got: {out}");
}

#[test]
fn dup_with_var_target_at_runtime() {
    // 2>&$FD with FD=1 — target Word has a Var part; expansion yields "1".
    let (out, _) = run("FD=1 sh -c 'echo varfd >&2' 2>&$FD\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "varfd"), "got: {out}");
}
