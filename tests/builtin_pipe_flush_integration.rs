//! v129: a forked builtin stage must flush its trailing partial line; the parent
//! must flush before forking so nothing is duplicated (M-118).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn builtin_unterminated_piped_not_truncated() {
    let (out, _e, _c) = run("printf \"%s\" abc | cat\n");
    assert_eq!(out, "abc", "out: {out:?}");
}

#[test]
fn builtin_only_last_line_unterminated_piped() {
    let (out, _e, _c) = run("printf \"x\\ny\\nz\" | cat\n");
    assert_eq!(out, "x\ny\nz", "out: {out:?}");
}

#[test]
fn builtin_unterminated_in_subshell() {
    let (out, _e, _c) = run("( printf x )\n");
    assert_eq!(out, "x", "out: {out:?}");
}

#[test]
fn no_duplication_parent_partial_then_piped_builtin() {
    let (out, _e, _c) = run("printf x; printf y | cat\n");
    assert_eq!(out, "xy", "out: {out:?}");
}

#[test]
fn terminated_builtin_unchanged() {
    let (out, _e, _c) = run("echo hello | cat\n");
    assert_eq!(out, "hello\n", "out: {out:?}");
}

#[test]
fn capture_subst_unaffected() {
    let (out, _e, _c) = run("v=$(printf \"%s\" abc); echo \"[$v]\"\n");
    assert_eq!(out, "[abc]\n", "out: {out:?}");
}

#[test]
fn loop_of_builtins_unterminated_piped() {
    let (out, _e, _c) = run("for i in 1 2 3; do printf \"$i\"; done | cat\n");
    assert_eq!(out, "123", "out: {out:?}");
}

// NOTE: the external_ordering_* tests require /usr/bin/printf (GNU coreutils
// external) — present on Linux/macOS/BSD but absent on some minimal/musl images.
#[test]
fn external_ordering_piped() {
    let (out, _e, _c) = run("printf x; /usr/bin/printf y | cat\n");
    assert_eq!(out, "xy", "out: {out:?}");
}

#[test]
fn external_ordering_bare() {
    let (out, _e, _c) = run("printf x; /usr/bin/printf y\n");
    assert_eq!(out, "xy", "out: {out:?}");
}

#[test]
fn external_ordering_in_subshell() {
    let (out, _e, _c) = run("printf x; ( /usr/bin/printf y )\n");
    assert_eq!(out, "xy", "out: {out:?}");
}
