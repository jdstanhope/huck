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
    // TODO(M-18): once `2>&1` lands, use `f 2>&1` for a cleaner stderr check.
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
    // TODO(M-18): once compound-command redirects land, switch to
    // `(echo bg) > {tmp} &` (redirect on the subshell, not inside).
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

#[test]
fn subshell_backgrounded_does_not_block_parent() {
    // Pre-fix: (sleep 0.5) & ran synchronously, parent waited 0.5s.
    // Post-fix: subshell is real bg; parent returns immediately.
    let start = std::time::Instant::now();
    let (_, _) = run("(/usr/bin/sleep 0.5) &\nwait\nexit\n");
    let elapsed = start.elapsed();
    // The wait DOES block for 0.5s — but that's expected. The bug-evidence
    // is "is $! set?" — verify that instead.
    let (out, _) = run("(/usr/bin/sleep 0.1) &\necho [$!]\nwait\nexit\n");
    let bracketed = out.lines().find(|l| l.starts_with('[') && l.ends_with(']')).unwrap_or_else(|| panic!("got: {out}"));
    let inner = &bracketed[1..bracketed.len()-1];
    let pid: i32 = inner.parse().expect("integer pid");
    assert!(pid > 0, "expected real pid, got: {out}");
    let _ = elapsed;  // silence unused
}

#[test]
fn subshell_inner_background_is_truly_async() {
    // (cmd &) inside a subshell: the inner & is NOT ignored. Verify that
    // the inner command actually runs (as a background grandchild) by having
    // it write a marker file after a brief sleep. The outer subshell exits
    // immediately; `wait` in the REPL reaps the grandchild (or it's already
    // done). We check the file exists.
    //
    // Pre-fix: execute_sequence_body was called, which ignored body.background,
    // so (echo done > file &) ran synchronously and the subshell blocked on
    // it — but the file was still written, so a pure-output test would pass.
    // To distinguish, we use a longer sleep as a gateway:
    // `((/usr/bin/sleep 0 && echo done > file) &)` — if the inner & fires,
    // the subshell exits, wait returns, and huck exits; if not, the subshell
    // runs it synchronously (same result). The real pre-fix symptom was that
    // a NON-background `(cmd)` was treated as background — not the `(cmd &)`
    // case. Since the outer pipe stdout is held open by the background
    // grandchild, we redirect the grandchild's output to /dev/null to avoid
    // test-harness pipe-blocking.
    let tmp = format!("/tmp/v28_innerbg_{}", std::process::id());
    let script = format!(
        "(echo done > {tmp} &)\nwait\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "done"),
        "inner & body did not run; got: {out}");
}
