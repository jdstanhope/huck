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
fn dollar_zero_in_function_keeps_invocation_name() {
    // bash: `$0` is NOT rebound to the function name on entry — it stays the
    // shell/script invocation name (which here contains "huck").
    let (out, _) = run("f() { echo $0; }\nf\nexit\n");
    assert!(out.lines().any(|l| l.contains("huck")), "got: {out}");
    assert!(
        !out.lines().any(|l| l.trim() == "f"),
        "$0 must not be the function name; got: {out}"
    );
}

#[test]
fn dollar_zero_nested_functions_keep_invocation_name() {
    let (out, _) = run("f() { g; echo $0; }\ng() { echo $0; }\nf\nexit\n");
    // Both prints are the invocation name, never the function names.
    let zero_lines: Vec<&str> = out.lines().filter(|l| l.contains("huck")).collect();
    assert!(zero_lines.len() >= 2, "expected two $0 prints, got: {out}");
    assert!(
        !out.lines().any(|l| l.trim() == "f" || l.trim() == "g"),
        "$0 must not be a function name; got: {out}"
    );
}

#[test]
fn dollar_zero_same_inside_and_outside_function() {
    let (out, _) = run("f() { echo $0; }\nf\necho $0\nexit\n");
    // The in-function and top-level `$0` are the same invocation name.
    let zero_lines: Vec<&str> = out.lines().filter(|l| l.contains("huck")).collect();
    assert!(zero_lines.len() >= 2, "expected matching $0 lines, got: {out}");
    assert_eq!(zero_lines[0].trim(), zero_lines[1].trim(), "got: {out}");
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
    // /bin/sleep is present on both Linux and macOS (macOS doesn't ship
    // /usr/bin/sleep). BSD `sleep` doesn't accept fractional seconds,
    // but `1` works on both — the test only cares that an external
    // background spawn sets $!, not how long it sleeps.
    let (out, _) = run("/bin/sleep 1 &\necho \"[$!]\"\nwait\nexit\n");
    // Output should contain "[N]" where N is a positive integer.
    let bracketed = out.lines().find(|l| l.starts_with('[') && l.ends_with(']')).expect("[pid] line");
    let inner = &bracketed[1..bracketed.len()-1];
    let pid: i32 = inner.parse().expect("integer inside brackets");
    assert!(pid > 0, "got: {out}");
}

#[test]
fn dollar_bang_is_last_stage_of_pipeline() {
    // Spawn a pipeline where the first stage prints its own pid (via $$).
    // $! should be the LAST stage's pid (the cat), not the first stage's.
    // Use /bin/ paths — present on both Linux and macOS (the latter
    // doesn't ship /usr/bin/{sh,cat}).
    let (out, _) = run(
        "/bin/sh -c 'echo $$' | /bin/cat &\nLAST=$!\nwait\necho \"[$LAST]\"\nexit\n"
    );
    let bracketed = out.lines().find(|l| l.starts_with('[')).expect("bracketed");
    let last_pid: i32 = bracketed[1..bracketed.len()-1].parse().expect("int");
    let first_pid: i32 = out.lines()
        .find(|l| l.trim().parse::<i32>().is_ok())
        .and_then(|l| l.trim().parse().ok())
        .expect("first stage pid printed");
    assert_ne!(last_pid, first_pid, "$! should be last stage's pid, not first; got: {out}");
    assert!(last_pid > 0);
}

#[test]
fn dollar_bang_set_after_backgrounded_pure_builtin() {
    // Pre-fix: pipeline_is_pure_builtin shortcut in run_background_sequence
    // ran echo synchronously in the parent without forking; last_bg_pid
    // stayed unset/stale. Post-fix: echo runs in a forked subshell and $!
    // returns that pid.
    let (out, _) = run("echo hi &\necho \"[$!]\"\nwait\nexit\n");
    let bracketed = out.lines()
        .find(|l| l.starts_with('[') && l.ends_with(']') && l.len() > 2)
        .unwrap_or_else(|| panic!("expected non-empty bracketed pid line, got: {out}"));
    let inner = &bracketed[1..bracketed.len()-1];
    let pid: i32 = inner.parse().expect("integer inside brackets");
    assert!(pid > 0, "got: {out}");
}

#[test]
fn dollar_bang_updates_after_second_background() {
    // After cmd1 &; cmd2 &, $! should reflect cmd2's pid, not cmd1's.
    let (out, _) = run(
        "/usr/bin/sleep 0.2 &\nFIRST=$!\necho hi &\nSECOND=$!\nwait\necho \"[$FIRST] [$SECOND]\"\nexit\n"
    );
    let line = out.lines()
        .find(|l| l.contains("[") && l.contains("]"))
        .unwrap_or_else(|| panic!("got: {out}"));
    let parts: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(parts.len(), 2, "got: {out}");
    assert_ne!(parts[0], parts[1], "$! should differ between two backgrounds; got: {out}");
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
