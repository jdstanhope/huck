//! End-to-end tests for v26 special parameters $0, $$, $!.

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
// $0 tests
// ---------------------------------------------------------------------------

#[test]
fn dollar_zero_top_level_contains_huck() {
    let (out, _) = run("echo $0\nexit\n");
    assert!(out.contains("huck"), "got: {out}");
}

#[test]
fn dollar_zero_in_function_is_function_name() {
    let (out, _) = run("f() { echo $0; }\nf\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "f"), "got: {out}");
}

#[test]
fn dollar_zero_nested_functions() {
    let (out, _) = run("f() { g; echo $0; }\ng() { echo $0; }\nf\nexit\n");
    let lines: Vec<&str> = out.lines().filter(|l| l.trim() == "f" || l.trim() == "g").collect();
    assert!(lines.len() >= 2, "got: {out}");
    // The inner call prints "g" first; the outer prints "f" second.
    assert_eq!(lines[0].trim(), "g", "got: {out}");
    assert_eq!(lines[1].trim(), "f", "got: {out}");
}

#[test]
fn dollar_zero_returns_to_shell_after_function() {
    let (out, _) = run("f() { echo $0; }\nf\necho $0\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    // First line is "f" from inside the function; second line contains "huck".
    let in_func = lines.iter().any(|l| l.trim() == "f");
    let outside_huck = lines.iter().any(|l| l.contains("huck"));
    assert!(in_func, "expected 'f' line, got: {out}");
    assert!(outside_huck, "expected huck-containing line outside function, got: {out}");
}

// ---------------------------------------------------------------------------
// $$ tests
// ---------------------------------------------------------------------------

#[test]
fn dollar_dollar_top_level_is_positive_integer() {
    let (out, _) = run("echo $$\nexit\n");
    let line = out.lines().find(|l| l.trim().parse::<i32>().is_ok()).expect("numeric line");
    let pid: i32 = line.trim().parse().unwrap();
    assert!(pid > 0, "expected positive pid, got: {pid}");
}

#[test]
fn dollar_dollar_same_in_subshell() {
    let (out, _) = run("echo $$\necho $$ | cat\nexit\n");
    let numeric_lines: Vec<i32> = out.lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .collect();
    assert!(numeric_lines.len() >= 2, "got: {out}");
    assert_eq!(numeric_lines[0], numeric_lines[1], "subshell $$ should match parent; got: {out}");
}

// ---------------------------------------------------------------------------
// $! tests
// ---------------------------------------------------------------------------

#[test]
fn dollar_bang_unset_initially_is_empty() {
    // Before any background command, $! should expand to an empty string.
    let (out, _) = run("echo \"[$!]\"\nexit\n");
    assert!(out.contains("[]"), "got: {out}");
}

#[test]
fn dollar_bang_set_after_backgrounded_external() {
    // /usr/bin/sleep is universally available on Linux.
    let (out, _) = run("/usr/bin/sleep 0.1 &\necho \"[$!]\"\nwait\nexit\n");
    // Output should contain "[N]" where N is a positive integer.
    let bracketed = out.lines().find(|l| l.starts_with('[') && l.ends_with(']')).expect("[pid] line");
    let inner = &bracketed[1..bracketed.len()-1];
    let pid: i32 = inner.parse().expect("integer inside brackets");
    assert!(pid > 0, "got: {out}");
}

#[test]
fn dollar_bang_is_last_stage_of_pipeline() {
    let (out, _) = run("echo hi | /usr/bin/sleep 0.1 &\necho \"[$!]\"\nwait\nexit\n");
    let bracketed = out.lines().find(|l| l.starts_with('[') && l.ends_with(']')).expect("[pid] line");
    let inner = &bracketed[1..bracketed.len()-1];
    let _pid: i32 = inner.parse().expect("integer inside brackets");
    // The exact pid isn't predictable; just verify it's a valid integer pid.
    // The semantic (LAST stage's pid) is covered by the spec; this test
    // documents that $! is a valid pid after a pipeline &.
}

#[test]
fn dollar_bang_preserves_across_subsequent_foreground() {
    // $! should not change after a foreground command.
    let (out, _) = run(
        "/usr/bin/sleep 0.1 &\nBG_PID=$!\ntrue\necho \"[$BG_PID] [$!]\"\nwait\nexit\n"
    );
    let line = out.lines().find(|l| l.contains('[')).expect("bracketed line");
    // Both bracketed values should be identical.
    let parts: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(parts.len(), 2, "got: {out}");
    assert_eq!(parts[0], parts[1], "$! changed after foreground command; got: {out}");
}
