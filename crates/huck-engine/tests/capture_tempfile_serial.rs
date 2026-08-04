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

/// `run().merge_stderr()` merges stderr into fd 1 via a REAL dup2 (#197 Stage 3).
/// Redirect this process's fd 1 to a temp file around the run and assert both
/// streams landed there in program order.
fn run_merge_dup2_sends_stderr_to_fd1() {
    use std::io::{Read, Seek, Write};
    use std::os::fd::AsRawFd;
    let _ = std::io::stdout().flush();
    let mut tf = tempfile::NamedTempFile::new().unwrap();
    let saved1 = unsafe { libc::dup(1) };
    assert!(saved1 >= 0);
    unsafe { libc::dup2(tf.as_raw_fd(), 1) };
    let mut e = Engine::new();
    let code = e.prepare("echo a; echo b >&2; echo c").merge_stderr().run();
    let _ = std::io::stdout().flush();
    unsafe {
        libc::dup2(saved1, 1);
        libc::close(saved1);
    }
    tf.as_file_mut().rewind().unwrap();
    let mut buf = String::new();
    tf.as_file_mut().read_to_string(&mut buf).unwrap();
    assert_eq!(code, 0);
    assert_eq!(buf, "a\nb\nc\n");
}

/// #197 Stage 3: `$(cat <<…)` runs the EXTERNAL `cat`; its output is captured
/// through the production fork + fd-level path. (Moved from the engine unit
/// tests, whose in-process thread-local capture can't intercept an external.)
/// A heredoc delimiter word spanning a `\<newline>` continuation forms `EOT4`.
fn heredoc_delim_line_continuation_in_comsub() {
    let mut e = Engine::new();
    let out = e.capture("x=$( cat <<\\EOT\\\n4\nd \\\ng\nEOT4\n)\necho \"$x\"");
    assert_eq!(out.stdout, "d \\\ng\n");
}

/// #197 Stage 3 (moved): a heredoc opened INSIDE `$( … )` whose `)` closes on
/// the opener line; the body is taken from the lines following the enclosing
/// command line (delayed heredoc across the comsub boundary).
fn heredoc_in_comsub_body_after_close() {
    let mut e = Engine::new();
    let out = e.capture("echo $(cat <<EOF)\nfoo\nbar\nEOF\n");
    assert_eq!(out.stdout, "foo bar\n");
    let out2 = e.capture("x=$(cat <<EOF)\none\ntwo\nEOF\necho \"[$x]\"");
    assert_eq!(out2.stdout, "[one\ntwo]\n");
}

/// #458 (moved from `engine::tests`): every `ExecBuilder` knob at once. The
/// `.stdin()` knob dup2s the process-global fd 0, so under the parallel `--lib`
/// harness a sibling's `< file` redirect could land on fd 0 first and `read x`
/// saw nothing — `got:\n` instead of `got:hello\n`. `STDIN_LOCK` did not help:
/// a shell-level redirect takes no such lock.
fn all_knobs_compose() {
    use std::time::Duration;
    let tmp = tempfile::tempdir().unwrap();
    let mut e = Engine::new();
    let out = e
        .prepare("read x; echo \"got:$x\"")
        .cwd(tmp.path())
        .restricted()
        .timeout(Duration::from_secs(2))
        .stdin(b"hello\n".to_vec())
        .capture();
    assert_eq!(out.exit_code, 0, "stderr={:?}", out.stderr);
    assert_eq!(out.stdout, "got:hello\n");
}

#[test]
fn capture_tempfile_checks_run_serially() {
    plain_stdout_captured();
    stdout_and_stderr_separated();
    merge_stderr_interleaves_into_stdout();
    exit_code_propagates();
    run_merge_dup2_sends_stderr_to_fd1();
    heredoc_delim_line_continuation_in_comsub();
    heredoc_in_comsub_body_after_close();
    all_knobs_compose();
}
