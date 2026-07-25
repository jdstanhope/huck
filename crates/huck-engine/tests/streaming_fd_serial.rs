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
    e.prepare("echo hi; echo err >&2")
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
    e.prepare("echo a; echo b >&2")
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
        .prepare("echo hi >&2")
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
        .prepare("declare -p NOPE_NOT_DEFINED 2>&1")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert!(
        lines.iter().any(|l| l.contains("NOPE_NOT_DEFINED")),
        "expected stderr-redirected-to-stdout line via callback, got {lines:?}"
    );
}

/// A heredoc opened inside a command substitution, where the heredoc close
/// delimiter is adjacent to the `$()` / backtick close. Moved here from
/// `engine.rs` (#297): the comsub capture swaps a process-global fd around an
/// EXTERNAL `cat`, so a sibling test's libtest output landing in that window
/// truncated the captured body to empty.
fn heredoc_in_comsub_eof_adjacency_expands() {
    let mut e = Engine::new();
    // comsub-eof0: `EOF )` (delimiter, space, `)`).
    assert_eq!(
        e.capture("foo=$(cat <<EOF\nhi\nEOF )\necho $foo").stdout,
        "hi\n"
    );
    // comsub-eof1: heredoc inside a BACKTICK (the former panic case).
    assert_eq!(
        e.capture("foo=`cat <<EOF\nhi\nEOF`\necho $foo").stdout,
        "hi\n"
    );
    // comsub-eof4: `EOF)` (no space before the `)`).
    assert_eq!(
        e.capture("e=$(cat <<EOF\ncontents\nEOF)\necho $e").stdout,
        "contents\n"
    );
}

/// v269 T3b regression: a builtin error emitted under `$(... 2>&1)` must reach
/// the CALLER's writer (the executor's in-memory route_err_to_out swap for the
/// bare-builtin redirect), not the thread-local sink — sh_error_to!, not
/// sh_error!. Verified bug (pre-fix): `x=$(cd /nonexistent 2>&1); echo "$x"`
/// printed an empty string instead of capturing `cd`'s diagnostic.
///
/// Moved here from `engine.rs` (#297) — same process-global fd swap, same race.
fn cmdsub_bare_builtin_2to1_capture_is_nonempty() {
    let mut e = Engine::new();
    let out = e
        .prepare(r#"x=$(cd /nonexistent_xyz_engine_test 2>&1); echo "$x""#)
        .capture();
    assert_eq!(out.stderr, "");
    assert!(
        !out.stdout.trim().is_empty(),
        "expected the cd error to be captured, got empty stdout"
    );
    assert!(
        out.stdout.contains("No such file or directory"),
        "expected the captured cd diagnostic body, got stdout=[{:?}]",
        out.stdout
    );
}

/// Callbacks fire in REAL TIME for an external child, not batched at exit.
/// Moved here from `engine.rs` (#297): it both swaps a process-global fd and
/// asserts on wall-clock gaps, so a parallel harness could drop a line (the
/// observed failure) or inflate the gap under load.
fn on_stdout_line_external_real_time() {
    use std::time::{Duration, Instant};
    let mut timestamps: Vec<Instant> = Vec::new();
    let mut e = Engine::new();
    let _ = e
        .prepare("/bin/sh -c 'echo first; sleep 0.1; echo second'")
        .on_stdout_line(|_line| timestamps.push(Instant::now()))
        .capture();
    assert_eq!(timestamps.len(), 2);
    let gap = timestamps[1].duration_since(timestamps[0]);
    assert!(
        gap >= Duration::from_millis(50),
        "expected ~100ms gap, got {gap:?}"
    );
    assert!(gap <= Duration::from_secs(2), "gap too large: {gap:?}");
}

/// Sandbox blocks ESCAPE, not local work — the one place it deliberately
/// diverges from bash's rbash, which refuses every file target. See
/// `docs/superpowers/specs/2026-07-20-restricted-policy-design.md`.
///
/// Moved here from `engine.rs` (#297): CWD_LOCK serialized the process-global
/// cwd swap but not libtest, so a sibling's `test … ok` progress line landed
/// inside the captured bytes. Here the binary holds a single test, so no
/// concurrent libtest output exists while fd 1 is swapped.
fn sandbox_permits_relative_redirect() {
    let dir = std::env::temp_dir().join(format!("huck-v319-sandbox-rel.{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mut e = Engine::new();
    let out = e
        .prepare("echo hi > local_log; cat local_log")
        .cwd(&dir)
        .restricted()
        .capture();
    assert_eq!(out.stdout, "hi\n", "stderr: {:?}", out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `.timeout()` deadline still delivers callback lines emitted before it
/// fires, and the call reports 124. Moved here from `engine.rs` (#297): a 200ms
/// deadline over an external child plus a callback fd swap returned 1 instead
/// of 124 under a loaded parallel harness.
fn on_stdout_line_with_timeout_fires_during_run() {
    use std::time::Duration;
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let code = e
        .prepare("/bin/sh -c 'echo before; sleep 5'")
        .timeout(Duration::from_millis(200))
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture()
        .exit_code;
    assert_eq!(code, 124);
    assert_eq!(lines, vec!["before"]);
}

/// Bare-builtin in-memory routing (Task 7): a builtin's `>&2` under stderr
/// capture lands in the stderr buffer, and `2>&1` folds stderr writes into the
/// stdout buffer. Moved here from `engine.rs` (#297) — it reads back through
/// the real fd 1, which a concurrently-forking sibling can invalidate.
fn capture_bare_dup_to_one_routes_to_stdout_sink() {
    // route_out_to_err
    let mut e = Engine::new();
    let out = e.prepare("echo out; echo err >&2").capture();
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");

    // route_err_to_out: use a builtin whose primary output goes to fd 2 —
    // `declare -p UNSET_NAME` writes the "not found" diagnostic there.
    let mut e = Engine::new();
    let out = e.prepare("declare -p NOPE_NOT_DEFINED 2>&1").capture();
    assert_eq!(out.stderr, "");
    assert!(
        out.stdout.contains("NOPE_NOT_DEFINED"),
        "got stdout=[{:?}]",
        out.stdout
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
    heredoc_in_comsub_eof_adjacency_expands();
    cmdsub_bare_builtin_2to1_capture_is_nonempty();
    on_stdout_line_external_real_time();
    sandbox_permits_relative_redirect();
    on_stdout_line_with_timeout_fires_during_run();
    capture_bare_dup_to_one_routes_to_stdout_sink();
}
