//! Integration test harness for v25 Task 3's fork helper.
//!
//! The substantive unit test lives in `src/executor.rs::tests::
//! fork_and_run_in_subshell_echo_stage_writes_to_pipe` (exercises the helper
//! directly via a pipe pair).
//!
//! The end-to-end test below is kept `#[ignore]`'d until Task 5 wires
//! `fork_and_run_in_subshell` into `run_multi_stage`. Once Task 5 lands,
//! this file will be superseded by `tests/pipeline_subshell_integration.rs`
//! and deleted in Task 6.

use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs a huck script and returns (stdout, stderr).
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

/// End-to-end smoke test: once Task 5 wires the fork helper into
/// run_multi_stage, `echo hi | cat` should print "hi" and exit 0.
/// Until then this is #[ignore]'d; the in-process unit test in
/// executor::tests covers the helper itself.
#[test]
#[ignore = "Task 5 not yet implemented — fork helper not wired into run_multi_stage"]
fn fork_runs_builtin_and_parent_reads_output() {
    let (out, _err) = run("echo hi | cat\nexit\n");
    assert!(out.contains("hi"), "expected 'hi' in output, got: {out:?}");
}
