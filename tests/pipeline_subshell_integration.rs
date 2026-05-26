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
// Task 6 — full spec test table
// ---------------------------------------------------------------------------

#[test]
fn pipeline_while_loop_as_stage() {
    // huck lacks `read`, so we can't use `while IFS= read -r x; do …; done`.
    // Instead: `seq 1 3 | while true; do cat; break; done` — the while body
    // runs `cat` which drains all of stdin (the pipe from seq), then `break`
    // exits the loop.  The subshell's stdout is the pipe to the implicit cat
    // on the right, so all three lines pass through.
    let (out, _) = run("seq 1 3 | while true; do cat; break; done\nexit\n");
    assert!(out.contains("1"), "expected '1' in output, got: {out}");
    assert!(out.contains("2"), "expected '2' in output, got: {out}");
    assert!(out.contains("3"), "expected '3' in output, got: {out}");
}

#[test]
fn pipeline_for_loop_as_stage() {
    // The for loop's body echoes its own loop variable; it ignores stdin (which
    // comes from `echo hi`).  The point is that a for-loop IS a valid pipeline
    // stage and executes in a forked subshell without error.
    let (out, _) = run("echo hi | for x in a b; do echo $x; done\nexit\n");
    assert!(out.contains("a"), "expected 'a' in output, got: {out}");
    assert!(out.contains("b"), "expected 'b' in output, got: {out}");
}

#[test]
fn pipeline_case_as_stage() {
    // `case` as a pipeline stage: stdin comes from `echo a` but the case
    // matches on a literal word.  The point is that case IS executable as a
    // stage; its output ("MATCHED") appears on the pipeline's stdout.
    let (out, _) = run("echo a | case foo in foo) echo MATCHED;; esac\nexit\n");
    assert!(out.contains("MATCHED"), "got: {out}");
}

#[test]
fn pipeline_function_def_as_stage_is_noop() {
    // `echo hi | f() { :; }` — the function definition runs in a subshell.
    // The subshell just registers `f` (into the child's table) then exits 0.
    // No output is produced.  The pipeline as a whole exits 0.
    let (out, err) = run("echo hi | f() { :; }\necho exit_was=$?\nexit\n");
    // No output from the pipeline itself.
    assert!(!out.contains("hi"), "stdin leaked to output: {out}");
    // The shell continues and the exit status is 0.
    assert!(out.contains("exit_was=0"), "expected exit_was=0, got: {out}\nstderr: {err}");
}

#[test]
fn pipeline_builtin_side_effect_does_not_leak() {
    // `cd /tmp | true` — cd runs in a subshell; the parent's cwd is unchanged.
    let orig_cwd = std::env::current_dir().unwrap();
    let (out, _) = run("cd /tmp | true\npwd\nexit\n");
    // The pwd output must be the original working directory, not /tmp.
    let cwd_str = orig_cwd.to_string_lossy();
    assert!(
        out.trim_end().ends_with(cwd_str.as_ref()),
        "expected cwd={cwd_str}, got: {out}"
    );
}

#[test]
fn pipeline_var_assignment_does_not_leak() {
    // `FOO=outer; FOO=inner true | cat; echo $FOO` → outer
    // The inline assignment `FOO=inner` on `true` is applied before the fork
    // and restored in the parent immediately after spawn (v23 scoping).
    // The parent-side restore means the parent's FOO stays "outer".
    let (out, _) = run("FOO=outer\nFOO=inner true | cat\necho $FOO\nexit\n");
    assert!(out.contains("outer"), "expected 'outer', got: {out}");
    assert!(!out.contains("inner"), "unexpected 'inner' in output: {out}");
}

#[test]
fn pipeline_exit_in_first_stage_does_not_exit_shell() {
    // `exit 5 | cat` — the `exit 5` runs in a forked subshell; it exits that
    // subshell with status 5, but the parent shell keeps running.
    let (out, _) = run("exit 5 | cat\necho still-here\nexit\n");
    assert!(out.contains("still-here"), "shell exited early, got: {out}");
}

#[test]
fn pipeline_compound_with_redirect() {
    // Heredoc inside the compound body, compound stage as the first pipeline
    // stage, grep as the second.
    // `if true; then cat <<EOF; fi | grep foo`
    //   foo
    //   bar
    //   EOF
    // → output contains "foo" but not "bar".
    let (out, _) = run("if true; then cat <<EOF; fi | grep foo\nfoo\nbar\nEOF\nexit\n");
    assert!(out.contains("foo"), "expected 'foo', got: {out}");
    assert!(!out.contains("bar"), "unexpected 'bar' leaked, got: {out}");
}

#[test]
fn pipeline_function_inherits_inline_assignment() {
    // `myfunc() { echo got:$FOO; }; FOO=hi myfunc | cat`
    // The inline assignment `FOO=hi` is applied before the fork so the child
    // (which runs `myfunc`) sees FOO=hi.
    let (out, _) = run("myfunc() { echo got:$FOO; }\nFOO=hi myfunc | cat\nexit\n");
    assert!(out.contains("got:hi"), "expected 'got:hi', got: {out}");
}

#[test]
fn pipeline_three_stages_compound_middle() {
    // Three-stage pipeline where the middle stage is a brace group.
    // `echo hi | { sed s/h/H/; } | cat` → "Hi"
    let (out, _) = run("echo hi | { sed s/h/H/; } | cat\nexit\n");
    assert!(out.contains("Hi"), "expected 'Hi', got: {out}");
    assert!(!out.contains("hi\n"), "unexpanded 'hi' present: {out}");
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
