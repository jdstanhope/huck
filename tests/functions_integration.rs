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
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn basic_function_definition_and_call() {
    let (out, _) = run("f() { echo hi; }\nf\nexit\n");
    assert!(out.lines().any(|l| l == "hi"), "stdout: {out}");
}

#[test]
fn function_with_positional_args() {
    let (out, _) = run("add() { echo $(($1 + $2)); }\nadd 3 4\nexit\n");
    assert!(out.lines().any(|l| l == "7"), "stdout: {out}");
}

#[test]
fn dollar_hash_is_argument_count() {
    let (out, _) = run("f() { echo n=$#; }\nf a b c d\nexit\n");
    assert!(out.lines().any(|l| l == "n=4"), "stdout: {out}");
}

#[test]
fn dollar_at_unquoted_word_splits() {
    let (out, _) = run("f() { for x in $@; do echo i-$x; done; }\nf alpha beta gamma\nexit\n");
    assert!(out.lines().any(|l| l == "i-alpha"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "i-beta"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "i-gamma"), "stdout: {out}");
}

#[test]
fn dollar_at_quoted_preserves_args() {
    // "$@" preserves each arg as its own field, even if it contains spaces.
    let (out, _) = run(
        "f() { for x in \"$@\"; do echo i=$x; done; }\nf \"hello world\" foo\nexit\n",
    );
    assert!(out.lines().any(|l| l == "i=hello world"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "i=foo"), "stdout: {out}");
}

#[test]
fn dollar_star_quoted_joins() {
    let (out, _) = run("f() { echo \"all=$*\"; }\nf a b c\nexit\n");
    assert!(out.lines().any(|l| l == "all=a b c"), "stdout: {out}");
}

#[test]
fn return_with_status() {
    let (out, _) = run("f() { return 7; }\nf\necho status-$?\nexit\n");
    assert!(out.lines().any(|l| l == "status-7"), "stdout: {out}");
}

#[test]
fn return_exits_early() {
    let (out, _) = run(
        "f() { echo before; return; echo never; }\nf\necho after\nexit\n",
    );
    assert!(out.lines().any(|l| l == "before"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "never"), "return failed: {out}");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
}

#[test]
fn function_in_and_or_sequence() {
    let (out, _) = run("f() { return 0; }\nf && echo yes\nexit\n");
    assert!(out.lines().any(|l| l == "yes"), "stdout: {out}");
}

#[test]
fn function_recursion() {
    let script = "countdown() { if test $1 -le 0; then echo done; return; fi; echo $1; countdown $(( $1 - 1 )); }\ncountdown 3\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "3"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "2"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "done"), "stdout: {out}");
}

#[test]
fn function_shadows_regular_builtin() {
    // The function `echo` is defined; calling `echo X` should run the
    // function (which prints something different), not the builtin.
    let (out, _) = run("echo() { command_does_not_exist; }\necho should-be-silenced\nexit\n");
    // The literal "should-be-silenced" must NOT appear (the function body
    // doesn't echo its args).
    assert!(!out.lines().any(|l| l == "should-be-silenced"), "stdout: {out}");
}

#[test]
fn return_is_unshadowable() {
    let (out, _) = run(
        "return() { echo BAD; }\nf() { return 3; echo NEVER; }\nf\necho status-$?\nexit\n",
    );
    assert!(!out.lines().any(|l| l == "BAD"), "return called the user fn: {out}");
    assert!(!out.lines().any(|l| l == "NEVER"), "return did not exit f: {out}");
    assert!(out.lines().any(|l| l == "status-3"), "stdout: {out}");
}

#[test]
fn break_inside_function_targets_callers_loop() {
    // POSIX/bash: `break` inside a function affects the caller's loop.
    let script = "leave() { break; }\nfor x in a b c; do echo at-$x; leave; done\necho after\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "at-a"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "at-b"), "break did not exit caller's loop: {out}");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
}

#[test]
fn multiline_function_definition() {
    let script = "f() {\n  echo line1\n  echo line2\n}\nf\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "line1"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "line2"), "stdout: {out}");
}

#[test]
fn standalone_brace_group_runs_in_current_shell() {
    let (out, _) = run("{ x=brace; echo x-set; }\necho after-x=$x\nexit\n");
    assert!(out.lines().any(|l| l == "x-set"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "after-x=brace"), "no isolation: {out}");
}

#[test]
fn stray_return_at_top_level_is_harmless() {
    let (out, _) = run("return\necho still-alive\nexit\n");
    assert!(out.lines().any(|l| l == "still-alive"), "stdout: {out}");
}

#[test]
fn function_body_can_be_if() {
    let script = "test_arg() if test $1 = yes; then echo matched; else echo other; fi\ntest_arg yes\ntest_arg no\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "matched"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "other"), "stdout: {out}");
}
