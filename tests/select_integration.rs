//! Integration tests for v81 `select` loops and the M-24a no-`in` `for` fix.
//! Drives the `huck` binary via stdin and asserts on stdout/stderr/exit code.
//! Only tests that do NOT require typing a menu choice are covered here;
//! the interactive pick path is verified by the pty suite.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
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
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

// ── M-24a: for no-`in` positionals ───────────────────────────────────────────

#[test]
fn for_no_in_iterates_positionals() {
    // `for x;` with no `in` list should iterate over the current "$@".
    let script = r#"set -- a b c; for x; do printf "%s " "$x"; done; echo
"#;
    let (out, _err, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "a b c \n");
}

#[test]
fn for_empty_in_iterates_nothing() {
    // Explicit `in` with no words — body never runs, script continues.
    let script = "set -- a b c; for x in ; do echo never; done; echo after\n";
    let (out, _err, code) = run_huck(script);
    assert_eq!(code, 0);
    assert!(out.contains("after"), "expected 'after' in stdout: {out:?}");
    assert!(
        !out.contains("never"),
        "body must not run with empty in-list: {out:?}"
    );
}

// ── select: no-interactive-read paths ────────────────────────────────────────

#[test]
fn select_empty_in_runs_no_body() {
    // Empty `select x in ;` — no menu, no body, script continues.
    let script = "select x in ; do echo never; done; echo after\n";
    let (out, _err, code) = run_huck(script);
    assert_eq!(code, 0);
    assert!(out.contains("after"), "expected 'after' in stdout: {out:?}");
    assert!(
        !out.contains("never"),
        "body must not run with empty in-list: {out:?}"
    );
}

#[test]
fn select_eof_prints_menu_to_stderr_no_body() {
    // With no extra input after the script, select's `read` hits EOF.
    // The menu is printed to stderr; the body never runs (EOF → exit loop).
    let script = "select x in a b; do echo ran; done\n";
    let (out, err, _code) = run_huck(script);
    assert!(
        err.contains("1) a"),
        "stderr should contain '1) a': {err:?}"
    );
    assert!(
        err.contains("2) b"),
        "stderr should contain '2) b': {err:?}"
    );
    assert!(
        err.contains("#? "),
        "stderr should contain the PS3 prompt '#? ': {err:?}"
    );
    assert!(
        !out.contains("ran"),
        "body must not run when stdin is EOF at prompt time: {out:?}"
    );
}
