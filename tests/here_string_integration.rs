//! End-to-end tests for v27 here-strings.

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
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn here_string_simple_word() {
    let (out, _) = run("cat <<< hello\nexit\n");
    assert!(out.contains("hello"), "got: {out}");
}

#[test]
fn here_string_quoted_word() {
    let (out, _) = run("cat <<< \"hello world\"\nexit\n");
    assert!(out.contains("hello world"), "got: {out}");
}

#[test]
fn here_string_expands_var() {
    let (out, _) = run("FOO=hi\ncat <<< $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn here_string_expands_command_sub() {
    let (out, _) = run("cat <<< $(echo via-sub)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "via-sub"), "got: {out}");
}

#[test]
fn here_string_with_inline_assignment() {
    let (out, _) = run("FOO=val cat <<< $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "val"), "got: {out}");
}

#[test]
fn here_string_in_pipeline_stage() {
    let (out, _) = run("cat <<< marker | grep marker\nexit\n");
    assert!(out.contains("marker"), "got: {out}");
}

#[test]
fn here_string_empty_word() {
    // cat <<< "" produces just a newline; verify by piping to wc -c.
    let (out, _) = run("cat <<< \"\" | wc -c\nexit\n");
    // wc -c output is "1" (just the trailing \n).
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn here_string_no_split_with_spaces() {
    let (out, _) = run("FOO=\"a b c\"\ncat <<< $FOO\nexit\n");
    // Should appear as one line "a b c", NOT three separate lines.
    assert!(out.lines().any(|l| l.trim() == "a b c"), "got: {out}");
}

#[test]
fn here_string_last_wins_over_file() {
    let tmp = format!("/tmp/huck_v27_lastwins_{}", std::process::id());
    let script = format!("echo wrong > {tmp}\ncat <{tmp} <<< right\nrm {tmp}\nexit\n");
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "right"), "got: {out}");
    assert!(
        !out.contains("wrong"),
        "file content leaked through; got: {out}"
    );
}

#[test]
fn here_string_trailing_newline_present() {
    // cat <<< hi produces "hi\n" — piping to wc -l should report 1 line.
    let (out, _) = run("cat <<< hi | wc -l\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn here_string_dollar_question_snapshot() {
    // After `false`, $? = 1. The here-string's expansion should see 1
    // (B-07 snapshot semantics via expand_assignment).
    let (out, _) = run("false\ncat <<< $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn here_string_single_quoted_no_expand() {
    // Single quotes prevent $FOO expansion; child sees literal "$FOO".
    let (out, _) = run("FOO=hi\ncat <<< '$FOO'\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "$FOO"), "got: {out}");
}

#[test]
fn here_string_backgrounded() {
    // Background a here-string redirected to a temp file; verify the
    // file contents include the body.
    let tmp = format!("/tmp/huck_v27_bg_{}", std::process::id());
    let script = format!("cat <<< body > {tmp} &\nwait\ncat {tmp}\nrm -f {tmp}\nexit\n");
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "body"), "got: {out}");
}
