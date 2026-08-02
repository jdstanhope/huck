//! Serial isolation for the production temp-file `ExecBuilder::capture()`.
//!
//! Integration binaries build the NON-test lib, so `.capture()` here takes the
//! real production path (#197 Stage 2): it redirects the process-global fd 1/2
//! onto a temp file, runs with `Terminal` sinks, restores, and reads back. The
//! `--lib` unit tests deliberately use the in-memory `#[cfg(test)]` capture
//! (parallel-safe), so this binary is the only place the temp-file path is
//! exercised.
//!
//! Like `streaming_fd_serial.rs`, these run as ONE `#[test]` — while the real
//! fd 1/2 is redirected, libtest's own progress output for a concurrently
//! finishing test would land on the redirected descriptor and corrupt the
//! captured bytes. Being the sole test in this binary, no concurrent libtest
//! output exists to leak while the descriptors are swapped.

use huck_engine::Engine;

/// Plain stdout is captured; stderr stays empty.
fn plain_stdout_captured() {
    let mut e = Engine::new();
    let out = e.prepare("echo hello; echo world").capture();
    assert_eq!(out.stdout, "hello\nworld\n");
    assert_eq!(out.stderr, "");
    assert_eq!(out.exit_code, 0);
}

/// stdout and stderr are captured into separate fields, in program order each.
fn stdout_and_stderr_separated() {
    let mut e = Engine::new();
    let out = e
        .prepare("echo out1; echo err1 >&2; echo out2; echo err2 >&2")
        .capture();
    assert_eq!(out.stdout, "out1\nout2\n");
    assert_eq!(out.stderr, "err1\nerr2\n");
    assert_eq!(out.exit_code, 0);
}

/// `merge_stderr`: both streams interleave into one temp file (program order);
/// `Output.stderr` is empty.
fn merge_stderr_interleaves_into_stdout() {
    let mut e = Engine::new();
    let out = e
        .prepare("echo a; echo b >&2; echo c")
        .merge_stderr()
        .capture();
    assert_eq!(out.stdout, "a\nb\nc\n");
    assert_eq!(out.stderr, "");
    assert_eq!(out.exit_code, 0);
}

/// A non-zero exit status is reported through the temp-file path.
fn exit_code_propagates() {
    let mut e = Engine::new();
    let out = e.prepare("echo hi; exit 7").capture();
    assert_eq!(out.stdout, "hi\n");
    assert_eq!(out.exit_code, 7);
}

#[test]
fn capture_tempfile_checks_run_serially() {
    plain_stdout_captured();
    stdout_and_stderr_separated();
    merge_stderr_interleaves_into_stdout();
    exit_code_propagates();
}
