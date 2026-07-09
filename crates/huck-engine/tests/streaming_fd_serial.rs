//! Serial isolation for streaming/capture checks that swap a process-global
//! standard fd (1 or 2) around an IN-PROCESS builtin and then read the result
//! back (from a file or a line callback).
//!
//! These used to be `#[test]`s in `engine.rs`, but they are fundamentally unsafe
//! under a parallel test harness: while the real fd 1/2 is redirected, libtest's
//! own progress output (`test … ok`) for a concurrently-finishing test — or a
//! sibling test's fd close — lands on the redirected descriptor and corrupts the
//! captured bytes. This is latent on Linux (the race almost never lands in the
//! microsecond window) but reproducible on macOS. As `engine.rs` already notes
//! for the fork+exec tee tests (see #90): "No in-process lock fixes that …
//! running them in a separate integration-test binary is the only reliable
//! isolation." So they live here, as ONE `#[test]` whose checks run
//! sequentially — the sole test in this binary, so no concurrent libtest output
//! exists to leak while a descriptor is swapped.

use std::io::Read;

use huck_engine::Engine;

/// bash: `cmd >file 2>&1` — the file gets the bytes; nothing is captured.
fn capture_with_file_then_dup_to_one_lets_file_win() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    let mut e = Engine::new();
    let out = e.capture(&format!("echo HI > {path} 2>&1"));
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, "");
    let mut s = String::new();
    std::fs::File::open(&path)
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    assert_eq!(s, "HI\n");
}

/// Symmetric: `cmd 2>file >&2` — the file gets the bytes; nothing is captured.
fn capture_with_file_then_dup_to_two_lets_file_win() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    let mut e = Engine::new();
    let out = e.capture(&format!("echo HI 2> {path} >&2"));
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, "");
    let mut s = String::new();
    std::fs::File::open(&path)
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    assert_eq!(s, "HI\n");
}

/// `on_stderr_line` fires once per stderr line.
fn on_stderr_line_fires_per_line() {
    let mut out_lines: Vec<String> = Vec::new();
    let mut err_lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.exec("echo hi; echo err >&2")
        .on_stdout_line(|line| out_lines.push(line.to_string()))
        .on_stderr_line(|line| err_lines.push(line.to_string()))
        .capture();
    assert_eq!(out_lines, vec!["hi"]);
    assert_eq!(err_lines, vec!["err"]);
}

/// `merge_stderr()` routes stderr lines through the stdout stream.
fn on_stdout_line_merge_stderr_routes_through_stdout() {
    let mut out_lines: Vec<String> = Vec::new();
    let mut err_lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.exec("echo a; echo b >&2")
        .merge_stderr()
        .on_stdout_line(|line| out_lines.push(line.to_string()))
        .on_stderr_line(|line| err_lines.push(line.to_string()))
        .capture();
    assert!(out_lines.contains(&"a".to_string()));
    assert!(out_lines.contains(&"b".to_string()));
    assert!(err_lines.is_empty());
}

/// A builtin's `>&2` reaches an `on_stderr_line` callback (v207 fixup).
fn on_stderr_line_builtin_redirect_to_err() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let out = e
        .exec("echo hi >&2")
        .on_stderr_line(|line| lines.push(line.to_string()))
        .capture();
    assert_eq!(out.stderr, "hi\n");
    assert_eq!(lines, vec!["hi"]);
}

/// A builtin diagnostic redirected `2>&1` reaches an `on_stdout_line` callback.
fn on_stdout_line_builtin_redirect_2to1() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let _ = e
        .exec("declare -p NOPE_NOT_DEFINED 2>&1")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert!(
        lines.iter().any(|l| l.contains("NOPE_NOT_DEFINED")),
        "expected stderr-redirected-to-stdout line via callback, got {lines:?}"
    );
}

#[test]
fn streaming_fd_checks_run_serially() {
    capture_with_file_then_dup_to_one_lets_file_win();
    capture_with_file_then_dup_to_two_lets_file_win();
    on_stderr_line_fires_per_line();
    on_stdout_line_merge_stderr_routes_through_stdout();
    on_stderr_line_builtin_redirect_to_err();
    on_stdout_line_builtin_redirect_2to1();
}
