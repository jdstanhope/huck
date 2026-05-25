//! End-to-end tests for v24 here-documents. Spawn huck with piped stdin
//! so the full lex → classify → parse → execute path runs.

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

// ---------------------------------------------------------------------------
// Basic: simple body, no variable expansion
// ---------------------------------------------------------------------------

#[test]
fn heredoc_simple_expand_no_vars() {
    let (out, _) = run("cat <<EOF\nhello\nEOF\nexit\n");
    assert!(out.contains("hello"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Literal heredoc (quoted delimiter): body taken verbatim, no expansion
// ---------------------------------------------------------------------------

#[test]
fn heredoc_literal_no_expand() {
    // `<<'EOF'` — dollar sign must be preserved verbatim.
    let (out, _) = run("FOO=secret\ncat <<'EOF'\n$FOO\nEOF\nexit\n");
    assert!(out.contains("$FOO"), "got: {out}");
    assert!(!out.contains("secret"), "must not expand: {out}");
}

// ---------------------------------------------------------------------------
// Expanding heredoc: variable interpolation
// ---------------------------------------------------------------------------

#[test]
fn heredoc_expand_var() {
    let (out, _) = run("FOO=hi\ncat <<EOF\n$FOO\nEOF\nexit\n");
    assert!(out.contains("hi"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Expanding heredoc: command substitution
// ---------------------------------------------------------------------------

#[test]
fn heredoc_expand_cmd_sub() {
    let (out, _) = run("cat <<EOF\n$(echo via-sub)\nEOF\nexit\n");
    assert!(out.contains("via-sub"), "got: {out}");
}

// ---------------------------------------------------------------------------
// <<- strip-tabs variant
// ---------------------------------------------------------------------------

#[test]
fn heredoc_strip_tabs() {
    // Body lines have leading tabs; close line has a leading tab.
    // After tab-stripping, cat sees "hello\n".
    let (out, _) = run("cat <<-EOF\n\t\thello\n\tEOF\nexit\n");
    assert!(out.contains("hello"), "got: {out}");
    // Leading tabs must be stripped, not passed through.
    assert!(!out.contains('\t'), "tabs should be stripped: {out}");
}

// ---------------------------------------------------------------------------
// Heredoc in a pipeline stage
// ---------------------------------------------------------------------------

#[test]
fn heredoc_in_pipeline() {
    // cat reads from heredoc, pipes through grep.
    let (out, _) = run("cat <<EOF | grep marker\nmarker\nother\nEOF\nexit\n");
    assert!(out.contains("marker"), "got: {out}");
    assert!(!out.contains("other"), "grep should filter 'other': {out}");
}

// ---------------------------------------------------------------------------
// Multiple heredocs on one command — last wins
// ---------------------------------------------------------------------------

#[test]
fn heredoc_multiple_per_command_last_wins() {
    // POSIX: when two stdin redirects appear, the last one wins.
    // `cat <<A <<B` — cat sees body B; body A is collected and discarded.
    let (out, _) = run("cat <<A <<B\nfirst\nA\nsecond\nB\nexit\n");
    assert!(out.contains("second"), "last body should win: {out}");
    assert!(!out.contains("first"), "first body should be discarded: {out}");
}

// ---------------------------------------------------------------------------
// Empty heredoc body
// ---------------------------------------------------------------------------

#[test]
fn heredoc_empty_body() {
    // `cat <<EOF\nEOF` — cat gets an immediately-closed stdin; no output.
    let (out, err) = run("cat <<EOF\nEOF\nexit\n");
    assert!(out.is_empty(), "expected empty output, got: {out}");
    assert!(err.is_empty(), "expected no errors, got: {err}");
}

// ---------------------------------------------------------------------------
// Inline assignment + heredoc: var set in prefix is visible in body expansion
// ---------------------------------------------------------------------------

#[test]
fn heredoc_with_inline_assignment_expand() {
    // FOO=hi is an inline assignment (temp-scope to cat).
    // The heredoc body `val=$FOO` should expand using that temp value.
    let (out, _) = run("FOO=hi cat <<EOF\nval=$FOO\nEOF\nexit\n");
    assert!(out.contains("val=hi"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Escape dollar sign in expanding heredoc
// ---------------------------------------------------------------------------

#[test]
fn heredoc_escape_dollar() {
    // `\$` inside an expanding heredoc → literal `$` (escape consumed).
    let (out, _) = run("cat <<EOF\n\\$NOT_EXPANDED\nEOF\nexit\n");
    assert!(out.contains("$NOT_EXPANDED"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Multi-line body
// ---------------------------------------------------------------------------

#[test]
fn heredoc_multi_line_body() {
    let (out, _) = run("cat <<EOF\nline1\nline2\nline3\nEOF\nexit\n");
    assert!(out.contains("line1"), "got: {out}");
    assert!(out.contains("line2"), "got: {out}");
    assert!(out.contains("line3"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Backgrounded command sees heredoc body
// ---------------------------------------------------------------------------

#[test]
fn heredoc_backgrounded_command_sees_body() {
    // Background cat reads its heredoc body and writes to a temp file
    // (since backgrounded stdout doesn't round-trip cleanly through the
    // test harness). Modelled after inline_assignment_backgrounded_external_command_sees_var.
    let tmp = format!("/tmp/huck_v24_bg_heredoc_{}", std::process::id());
    let script = format!(
        "cat <<EOF > {tmp} &\nbackground-test\nEOF\nwait\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.contains("background-test"), "got: {out}");
}
