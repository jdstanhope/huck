use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with `script` piped to stdin; returns (stdout, stderr).
fn run(script: &str) -> (String, String) {
    let (stdout, stderr, _) = run_with_status(script);
    (stdout, stderr)
}

/// Runs huck and also returns the decoded exit status code.
fn run_with_status(script: &str) -> (String, String, i32) {
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
fn multiline_if() {
    let (out, _) = run("if true\nthen\necho yes\nfi\nexit\n");
    assert!(out.lines().any(|l| l == "yes"), "stdout: {out}");
}

#[test]
fn multiline_if_else_taken() {
    let (out, _) = run("if false\nthen\necho a\nelse\necho b\nfi\nexit\n");
    assert!(out.lines().any(|l| l == "b"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "a"), "stdout: {out}");
}

#[test]
fn multiline_while() {
    let (out, _) = run("i=0\nwhile test $i -lt 3\ndo\necho n$i\ni=$((i+1))\ndone\nexit\n");
    for marker in ["n0", "n1", "n2"] {
        assert!(out.lines().any(|l| l == marker), "missing {marker}: {out}");
    }
}

#[test]
fn multiline_until() {
    let (out, _) = run("n=2\nuntil test $n -eq 0\ndo\necho u$n\nn=$((n-1))\ndone\nexit\n");
    assert!(out.lines().any(|l| l == "u2"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "u1"), "stdout: {out}");
}

#[test]
fn nested_loop_inside_if() {
    let script = "if true\nthen\ni=0\nwhile test $i -lt 2\ndo\necho x$i\ni=$((i+1))\ndone\nfi\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "x0"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "x1"), "stdout: {out}");
}

#[test]
fn quote_spanning_two_lines() {
    // The newline inside the quote is literal content.
    let (out, _) = run("echo \"line one\nline two\"\nexit\n");
    assert!(out.contains("line one\nline two"), "stdout: {out:?}");
}

#[test]
fn trailing_pipe_continues() {
    let (out, _) = run("echo hello |\ncat\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out}");
}

#[test]
fn trailing_andand_continues() {
    let (out, _) = run("true &&\necho reached\nexit\n");
    assert!(out.lines().any(|l| l == "reached"), "stdout: {out}");
}

#[test]
fn backslash_newline_joins_lines() {
    let (out, _) = run("echo one \\\ntwo \\\nthree\nexit\n");
    assert!(out.lines().any(|l| l == "one two three"), "stdout: {out}");
}

#[test]
fn eof_inside_unterminated_if_is_a_syntax_error() {
    let (_, err, code) = run_with_status("if true\nthen\necho hi\n");
    assert!(
        err.to_lowercase().contains("unexpected end of input"),
        "stderr: {err}"
    );
    assert_eq!(code, 2, "exit code");
}

#[test]
fn multiline_command_stored_as_single_history_line() {
    // After a multi-line `if`, `history` lists it collapsed onto one line.
    let (out, _) = run("if true\nthen\necho hi\nfi\nhistory\nexit\n");
    assert!(
        out.lines().any(|l| l.contains("if true; then echo hi; fi")),
        "history did not show the collapsed form; stdout: {out}"
    );
}
