//! PTY regression tests for M-104: a multi-stage pipeline inside a subshell
//! `( a | b )` must NOT deadlock when huck has a controlling terminal.
//!
//! These spawn huck under a real pseudo-terminal (via expectrl, the same
//! mechanism `tests/pty_interactive.rs` uses), feed a script, and wait for a
//! sentinel marker with a HARD per-read timeout. A hang therefore FAILS the
//! test (the marker never arrives within the timeout) rather than wedging the
//! suite — dropping the session at the end kills the wedged child. If PTY
//! allocation fails (restricted sandbox), the test logs a skip and passes — the
//! non-pty `subshell_pipeline_integration.rs` suite still catches a broken
//! binary.

use std::process::Command;
use std::time::Duration;

use expectrl::Expect;
use expectrl::session::OsSession;

/// Result of driving huck under a PTY: each requested `marker` is recorded as
/// `(marker, true)` if it appeared on the PTY stream within the timeout, or
/// `(marker, false)` if waiting for it timed out (a hang). `skipped` is true
/// when no PTY could be allocated.
struct PtyRun {
    markers: Vec<(String, bool)>,
    skipped: bool,
}

impl PtyRun {
    fn saw(&self, marker: &str) -> bool {
        self.skipped || self.markers.iter().any(|(m, ok)| m == marker && *ok)
    }
}

/// Drives huck under a PTY: sends each `(line, expect_marker)` pair (line
/// followed by CR) and, after each, waits for `expect_marker` to appear on the
/// PTY with the given per-read `timeout`. A timeout (deadlock) is recorded as a
/// missing marker rather than blocking the suite. The session is dropped at the
/// end, closing the master fd and killing any wedged child.
fn run_in_pty(steps: &[(&str, &str)], timeout: Duration) -> PtyRun {
    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("subshell_pipeline_pty: skipping — no PTY available: {e}");
            return PtyRun {
                markers: Vec::new(),
                skipped: true,
            };
        }
    };
    // Bound every read so a deadlock turns into a timeout, not a hang.
    session.set_expect_timeout(Some(timeout));

    let mut markers = Vec::with_capacity(steps.len());
    for (line, marker) in steps {
        if session.send(line).is_err() || session.send("\r").is_err() {
            markers.push((marker.to_string(), false));
            break;
        }
        if marker.is_empty() {
            continue;
        }
        let ok = session.expect(*marker).is_ok();
        markers.push((marker.to_string(), ok));
        if !ok {
            // A hang: don't keep sending into a wedged child.
            break;
        }
    }
    // Dropping `session` closes the master fd; a wedged child is killed.
    PtyRun {
        markers,
        skipped: false,
    }
}

#[test]
fn subshell_pipeline_does_not_hang_on_tty() {
    let run = run_in_pty(
        &[
            ("( echo hi | cat )", "hi"),
            ("echo DONE_MARK", "DONE_MARK"),
            ("exit", ""),
        ],
        Duration::from_secs(5),
    );
    assert!(
        run.saw("hi"),
        "missing pipeline output (subshell hung before producing 'hi')"
    );
    assert!(
        run.saw("DONE_MARK"),
        "subshell hung (no DONE_MARK within timeout)"
    );
}

#[test]
fn subshell_multistage_pipeline_does_not_hang_on_tty() {
    let run = run_in_pty(
        &[
            ("( echo hi | head -n 1 | tail -n 1 )", "hi"),
            ("echo DONE2", "DONE2"),
            ("exit", ""),
        ],
        Duration::from_secs(5),
    );
    assert!(
        run.saw("DONE2"),
        "multistage subshell hung (no DONE2 within timeout)"
    );
}
