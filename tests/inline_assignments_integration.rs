//! End-to-end tests for v23 inline assignments. Run `huck` as a subprocess
//! with piped stdin so the full lex/parse/execute path is exercised.

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
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

// ---------------------------------------------------------------------------
// External command: var is visible inside child, restored in shell after
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_external_command_sees_var() {
    // The external `env` prints all exported vars; grep filters to ^FOO=.
    let (out, _) = run("FOO=hi env | grep ^FOO=\nexit\n");
    assert!(out.contains("FOO=hi"), "got: {out}");
}

#[test]
fn inline_assignment_external_command_restores_after() {
    // FOO was unset before the prefix; shell restores it to unset after `true`.
    let (out, _) = run("unset FOO\nFOO=hi true\necho \"[$FOO]\"\nexit\n");
    assert!(out.contains("[]"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Left-to-right expansion: later RHS sees earlier assignment's value
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_left_to_right_visibility() {
    // A=1 is applied first; B=$A therefore expands to 1 inside the child.
    let (out, _) = run("A=1 B=$A env | grep -E '^[AB]='\nexit\n");
    assert!(out.contains("A=1"), "got: {out}");
    assert!(out.contains("B=1"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Restoration: prior state (unset / unexported) is correctly reinstated
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_unset_before_restores_to_unset() {
    // After `true` finishes, FOO is unset; `printenv FOO` exits 1.
    let (out, _) = run("unset FOO\nFOO=hi true\nprintenv FOO\necho status=$?\nexit\n");
    assert!(out.contains("status=1"), "got: {out}");
}

#[test]
fn inline_assignment_set_unexported_before_keeps_unexported_after() {
    // FOO=outer is a plain assignment (not exported). After FOO=inner true,
    // FOO reverts to its prior unexported state — env should not include it.
    let (out, _) = run("FOO=outer\nFOO=inner true\nenv | grep ^FOO= || echo not-exported\nexit\n");
    assert!(out.contains("not-exported"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Regular builtin: assignments restored after
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_regular_builtin_restores() {
    // `test` is a regular builtin; FOO=inner is temporary.
    let (out, _) = run("FOO=outer\nFOO=inner test -n \"$FOO\"\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "outer"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Special builtin: assignments persist
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_special_builtin_persists() {
    // `export` is a special builtin; FOO=val persists.
    let (out, _) = run("FOO=val export FOO\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "val"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Function calls: assignments do NOT persist (temporary scope, like bash 5.2)
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_function_call_does_not_persist() {
    // bash 5.2: a prefix assignment on a function call does NOT persist after
    // the function returns — only POSIX special builtins persist. FOO was unset
    // before the command, so `echo $FOO` prints an empty line afterward.
    // Body uses `true` (not `:` — huck has no `:` builtin yet).
    let (out, _) = run("myfunc() { true; }\nFOO=val myfunc\necho \"[$FOO]\"\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "[]"), "got: {out}");
    assert!(!out.lines().any(|l| l.trim() == "[val]"), "got: {out}");
}

#[test]
fn inline_assignment_function_mutation_clobbered_by_restore() {
    // The function's own write to the prefixed var is clobbered back to the
    // pre-command state (unset) by the snapshot/restore — bash-faithful.
    let (out, _) =
        run("myfunc() { FOO=\"$FOO-modified\"; }\nFOO=initial myfunc\necho \"[$FOO]\"\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "[]"), "got: {out}");
    assert!(
        !out.lines().any(|l| l.trim() == "[initial-modified]"),
        "got: {out}"
    );
}

// ---------------------------------------------------------------------------
// $? snapshot (B-07 reuse): RHS expansion sees pre-prefix $?, not post-command
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_dollar_question_snapshot() {
    // `false` sets $? = 1.  `FOO=$? env` should export FOO=1 to the child
    // because expand_assignment snapshots $? at expansion time (B-07).
    // We verify via the child's env rather than the shell's post-restore
    // state (since `env` is an external command, FOO is restored to unset
    // after it finishes).
    let (out, _) = run("false\nFOO=$? env | grep ^FOO=\nexit\n");
    assert!(out.contains("FOO=1"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Edge cases: empty RHS, tilde RHS, repeated name
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_empty_rhs() {
    // FOO= (empty RHS) is valid; child should see FOO= in its env.
    let (out, _) = run("FOO= env | grep ^FOO=\nexit\n");
    // Accept either "FOO=\n" or just "FOO=" at end of output.
    assert!(out.contains("FOO="), "got: {out}");
}

#[test]
fn inline_assignment_tilde_expands() {
    // HOME is set in the prefix first; FOO=~ expands using the just-set HOME.
    let (out, _) = run("HOME=/tmp/x FOO=~ env | grep ^FOO=\nexit\n");
    assert!(out.contains("FOO=/tmp/x"), "got: {out}");
}

#[test]
fn inline_assignment_repeated_name_restores_original() {
    // FOO=a FOO=b — child sees FOO=b; after restore, shell has FOO=outer.
    let (out, _) = run("FOO=outer\nFOO=a FOO=b true\necho $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "outer"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Backgrounded external commands: inline assignments reach the child
// ---------------------------------------------------------------------------

#[test]
fn inline_assignment_backgrounded_external_command_sees_var() {
    // Backgrounded externals must still see their inline assignments. Before
    // the fix, run_background_sequence skipped apply_inline_assignments and
    // the child's env was missing FOO entirely.
    //
    // We capture the child's env to a temp file (via shell redirection) since
    // backgrounded stdout doesn't round-trip cleanly through the test harness.
    let tmp = format!("/tmp/huck_bg_inline_test_{}", std::process::id());
    let script = format!("FOO=hi env > {tmp} &\nwait\ncat {tmp} | grep ^FOO=\nrm -f {tmp}\nexit\n");
    let (out, _) = run(&script);
    assert!(out.contains("FOO=hi"), "got: {out}");
}

// ---------------------------------------------------------------------------
// Regression: multi-assign speculative-peel iterator-drain bug (v30)
// ---------------------------------------------------------------------------

#[test]
fn multi_assign_with_trailing_semi_command_runs_both() {
    // Before the fix, `A=1 B=2 echo first; echo second` silently lost
    // `echo second` because iter.by_ref() drained the outer iterator into a
    // sub-iter, and tokens after the `;` in the sub-iter were dropped when
    // parse_pipeline_with_first returned.
    let (out, _) = run("A=1 B=2 echo first; echo second\nexit\n");
    let lines: Vec<&str> = out
        .lines()
        .filter(|l| *l == "first" || *l == "second")
        .collect();
    assert_eq!(lines, vec!["first", "second"], "got: {out}");
}

// ---------------------------------------------------------------------------
// Pipeline stage scoping (from Task 5 — kept here for completeness)
// ---------------------------------------------------------------------------

#[test]
fn pipeline_stage_inline_assignments_are_scoped_per_stage() {
    let (out, _) = run("FOO=stage1 env | FOO=stage2 grep ^FOO=\n");
    // grep's env has FOO=stage2; stage1's env had FOO=stage1.
    // `env` outputs all its env vars; `grep` filters to ^FOO=.
    // The line we expect is `FOO=stage1` because that's what `env`
    // saw — `grep` sees its own FOO=stage2 but only prints from stdin.
    assert!(out.contains("FOO=stage1"), "got: {out}");
    assert!(!out.contains("FOO=stage2"), "stage2 should not leak: {out}");
}
