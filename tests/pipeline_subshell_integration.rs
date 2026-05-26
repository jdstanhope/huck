//! End-to-end tests for v25 pipelines-as-subshells.
//!
//! Task 5 adds the three failing tests below; they are expected to FAIL until
//! run_multi_stage is rewritten around raw pipe fds with per-stage fork dispatch.
//! After the rewrite all three should pass, along with every pre-v25 test.

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
// Task 5 — three core integration tests (initially failing)
// ---------------------------------------------------------------------------

#[test]
fn pipeline_function_call_as_stage() {
    // Smallest "function in pipeline" test: myfunc wraps sed.
    let (out, _) = run("myfunc() { sed s/h/H/; }\necho hello | myfunc\nexit\n");
    assert!(out.contains("Hello"), "got: {out}");
}

#[test]
fn pipeline_if_clause_as_stage() {
    let (out, _) = run("echo hi | if true; then cat; fi\nexit\n");
    assert!(out.contains("hi"), "got: {out}");
}

#[test]
fn pipeline_brace_group_as_stage() {
    let (out, _) = run("echo hi | { cat; }\nexit\n");
    assert!(out.contains("hi"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Bug-fix tests: fd-lifecycle correctness in run_multi_stage
// ---------------------------------------------------------------------------

#[test]
fn pipeline_middle_stage_with_explicit_stdin_redirect_doesnt_corrupt_downstream() {
    // 3-stage pipeline where middle stage overrides stdin via heredoc.
    // The 3rd stage should see the middle stage's output, NOT the first
    // stage's output bleeding through a leaked pipe.
    let (out, _) = run("echo FIRST | cat <<EOF | grep MIDDLE\nMIDDLE\nEOF\nexit\n");
    assert!(out.contains("MIDDLE"), "got: {out}");
    assert!(!out.contains("FIRST"), "first-stage output leaked into pipeline 3: {out}");
}

#[test]
fn pipeline_capture_with_spawn_failure_doesnt_double_close() {
    // Command substitution containing a pipeline where a stage fails to
    // spawn (use a non-existent command in stage 2). Should produce a clean
    // exit, not a double-close abort/UB.
    let (out, _) = run("x=$(echo hi | /definitely/does/not/exist/binary)\necho [$x]\nexit\n");
    // Stage 2 fails to spawn; the substitution returns whatever it got from
    // stage 1's stdout via the capture pipe. The exact contents may vary;
    // we just verify huck didn't crash.
    assert!(out.contains("["), "got: {out}");
}
