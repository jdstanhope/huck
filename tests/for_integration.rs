use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with `script` piped to stdin, in working directory `dir`.
/// Returns (stdout, stderr).
fn run_in_dir(dir: &Path, script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .current_dir(dir)
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

/// Runs huck with `script` piped to stdin, in the test process's cwd.
fn run(script: &str) -> (String, String) {
    run_in_dir(Path::new("."), script)
}

#[test]
fn for_over_literal_list() {
    let (out, _) = run("for x in a b c; do echo v-$x; done\nexit\n");
    let got: Vec<&str> = out.lines().filter(|l| l.starts_with("v-")).collect();
    assert_eq!(got, vec!["v-a", "v-b", "v-c"], "stdout: {out}");
}

#[test]
fn for_empty_list_runs_zero_times() {
    let (out, _) = run("for x in; do echo NOPE; done\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "NOPE"), "stdout: {out}");
}

#[test]
fn for_no_in_runs_zero_times() {
    let (out, _) = run("for x; do echo NOPE; done\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "NOPE"), "stdout: {out}");
}

#[test]
fn for_over_command_substitution() {
    let (out, _) = run("for n in $(echo 1 2 3); do echo n$n; done\nexit\n");
    for marker in ["n1", "n2", "n3"] {
        assert!(out.lines().any(|l| l == marker), "missing {marker}: {out}");
    }
}

#[test]
fn for_word_splits_unquoted_variable() {
    let (out, _) = run("list=\"a b c\"\nfor x in $list; do echo i-$x; done\nexit\n");
    let got: Vec<&str> = out.lines().filter(|l| l.starts_with("i-")).collect();
    assert_eq!(got, vec!["i-a", "i-b", "i-c"], "stdout: {out}");
}

#[test]
fn for_over_glob() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();
    std::fs::write(dir.path().join("c.log"), "").unwrap();
    let (out, _) = run_in_dir(dir.path(), "for f in *.txt; do echo got-$f; done\nexit\n");
    assert!(out.lines().any(|l| l == "got-a.txt"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "got-b.txt"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "got-c.log"), "stdout: {out}");
}

#[test]
fn for_multiline() {
    let (out, _) = run("for x in a b\ndo\necho m-$x\ndone\nexit\n");
    assert!(out.lines().any(|l| l == "m-a"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "m-b"), "stdout: {out}");
}

#[test]
fn for_nested_inside_if() {
    let (out, _) = run("if true; then for x in a b; do echo f-$x; done; fi\nexit\n");
    assert!(out.lines().any(|l| l == "f-a"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "f-b"), "stdout: {out}");
}

#[test]
fn while_nested_inside_for() {
    let script =
        "for x in p q; do i=0; while test $i -lt 2; do echo $x$i; i=$((i+1)); done; done\nexit\n";
    let (out, _) = run(script);
    for marker in ["p0", "p1", "q0", "q1"] {
        assert!(out.lines().any(|l| l == marker), "missing {marker}: {out}");
    }
}

#[test]
fn for_break_exits_early() {
    let (out, _) =
        run("for x in a b c d; do if test $x = c; then break; fi; echo k-$x; done\nexit\n");
    assert!(out.lines().any(|l| l == "k-a"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "k-b"), "stdout: {out}");
    assert!(
        !out.lines().any(|l| l == "k-c"),
        "loop should have broken: {out}"
    );
}

#[test]
fn for_continue_skips_iteration() {
    let (out, _) =
        run("for x in a b c; do if test $x = b; then continue; fi; echo s-$x; done\nexit\n");
    assert!(out.lines().any(|l| l == "s-a"), "stdout: {out}");
    assert!(
        !out.lines().any(|l| l == "s-b"),
        "s-b should be skipped: {out}"
    );
    assert!(out.lines().any(|l| l == "s-c"), "stdout: {out}");
}

#[test]
fn for_variable_observable_after_loop() {
    let (out, _) = run("for x in one two three; do echo iter; done\necho final-$x\nexit\n");
    assert!(out.lines().any(|l| l == "final-three"), "stdout: {out}");
}

#[test]
fn for_invalid_variable_name_is_nonfatal_runtime_error() {
    // bash parses any word as the loop var and validates the identifier at
    // runtime: a bad name (`2x`) is a NON-FATAL "not a valid identifier" error
    // (body not run, the surrounding list continues — `still-alive` prints).
    let (out, err) = run("for 2x in a; do echo hi; done\necho still-alive\nexit\n");
    assert!(err.contains("not a valid identifier"), "stderr: {err}");
    assert!(
        !out.lines().any(|l| l == "hi"),
        "loop body must not run: {out}"
    );
    assert!(out.lines().any(|l| l == "still-alive"), "stdout: {out}");
}
