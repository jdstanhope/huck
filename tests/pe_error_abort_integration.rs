use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String, std::process::ExitStatus) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status,
    )
}

#[test]
fn error_if_unset_aborts_rest_of_sequence() {
    // `${X:?msg}; echo continued` — bash exits before echo runs.
    let (out, err, _) = run("${X:?missing}\necho continued\nexit\n");
    assert!(!out.lines().any(|l| l == "continued"), "stdout: {out}");
    assert!(err.contains("X: missing"), "stderr: {err}");
}

#[test]
fn error_if_unset_non_interactive_exits_shell() {
    // The script is `${X:?missing}\necho after\n` — huck should exit
    // with status 1 BEFORE reaching `echo after` (no `after` in stdout).
    let (out, err, status) = run("${X:?missing}\necho after\n");
    assert!(!out.lines().any(|l| l == "after"), "stdout should not have 'after': {out}");
    assert!(err.contains("X: missing"), "stderr: {err}");
    assert_eq!(status.code(), Some(1), "exit status should be 1, got {status:?}");
}

#[test]
fn error_if_unset_colon_treats_empty_as_unset() {
    // X is set to empty; `:?` treats empty as unset so it should fire.
    let (out, _err, status) = run("X=\"\"\n${X:?empty}\necho after\n");
    assert!(!out.lines().any(|l| l == "after"), "stdout: {out}");
    assert_eq!(status.code(), Some(1));
}

#[test]
fn error_if_unset_without_colon_only_aborts_when_unset() {
    // X is set to empty; `?` (no colon) treats empty as set, so it
    // should NOT fire.
    let (out, _err, _) = run("X=\"\"\n: ${X?empty}\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
}

#[test]
fn error_if_unset_when_set_passes_through() {
    let (out, _err, _) = run("X=hello\necho \"${X:?missing}\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out}");
}

#[test]
fn substring_negative_computed_length_aborts_and_exits() {
    let (out, err, status) = run("s=abc\necho \"[${s:0:-4}]\"\necho after\n");
    assert!(!out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(err.contains("substring expression < 0"), "stderr: {err}");
    assert_eq!(status.code(), Some(1));
}

#[test]
fn bad_arith_in_substring_aborts_and_exits() {
    // v178 (corrects a prior "stays non-fatal" claim): an arithmetic error in a
    // ${var:off:len} offset/length aborts the command with exit 1 — like the
    // substring-<0 case above — matching bash (`${s:@@@}` exits 1).
    let (out, err, status) = run("s=hello\necho \"[${s:@@@}]\"\necho after\n");
    assert!(!out.lines().any(|l| l == "[]"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(err.contains("error token is"), "stderr: {err}");
    assert_eq!(status.code(), Some(1));
}

#[test]
fn length_positional_in_function() {
    let (out, _err, _) = run("f() { echo ${#1}; }\nf hello\nexit\n");
    assert!(out.lines().any(|l| l == "5"), "stdout: {out}");
}

#[test]
fn length_at_star_match_hash_in_function() {
    let (out, _err, _) = run("f() { echo \"${#@},${#*},${#}\"; }\nf x y z\nexit\n");
    assert!(out.lines().any(|l| l == "3,3,3"), "stdout: {out}");
}

#[test]
fn error_if_unset_inside_subshell_does_not_kill_parent() {
    // The subshell's fatal flag stays in the cloned Shell; the parent's
    // flag is untouched. After the subshell exits non-zero, `echo after`
    // runs in the parent.
    let (out, _err, _) = run("(${X:?missing})\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
}

#[test]
fn error_if_unset_in_case_pattern_aborts() {
    // The case pattern contains ${X:?missing}. The expansion fires fatal;
    // the case statement should not execute any arm body, and subsequent
    // commands should not run.
    let (out, _err, status) = run("case foo in\n${X:?missing}) echo matched;;\nesac\necho after\n");
    assert!(!out.lines().any(|l| l == "matched"), "stdout should not have 'matched': {out}");
    assert!(!out.lines().any(|l| l == "after"), "stdout should not have 'after': {out}");
    assert_eq!(status.code(), Some(1));
}

#[test]
fn error_if_unset_in_redirect_target_aborts() {
    // The redirect target contains ${X:?missing}. The command should
    // not fork — no file created, no command runs, exit before `echo after`.
    let (out, _err, status) = run("echo hello > ${X:?missing}\necho after\n");
    assert!(!out.lines().any(|l| l == "hello"), "stdout should not have 'hello': {out}");
    assert!(!out.lines().any(|l| l == "after"), "stdout should not have 'after': {out}");
    assert_eq!(status.code(), Some(1));
}
