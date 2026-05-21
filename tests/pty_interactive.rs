//! PTY-based golden-path tests for huck's interactive features
//! (tab completion, history recall, Ctrl-C handling).
//!
//! These need a real pseudo-terminal so rustyline runs in interactive
//! mode. If PTY allocation fails (a restricted sandbox), each test
//! logs a skip notice and returns — a pass. A genuinely broken huck
//! binary is still caught by the piped-stdin integration suites.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use expectrl::session::OsSession;
use expectrl::{Eof, Expect};

// Keystroke encodings sent over the PTY master.
#[allow(dead_code)]
const TAB: &str = "\t";
const ENTER: &str = "\r";
#[allow(dead_code)]
const UP: &str = "\x1b[A";
#[allow(dead_code)]
const DOWN: &str = "\x1b[B";
#[allow(dead_code)]
const CTRL_C: &str = "\x03";
#[allow(dead_code)]
const CTRL_D: &str = "\x04";

/// Spawns the huck binary attached to a fresh PTY, in `cwd`, with the
/// given environment overrides applied on top of the inherited env.
/// Returns `None` (after logging) if PTY allocation fails.
fn try_spawn(cwd: &Path, env: &[(&str, &str)]) -> Option<OsSession> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    cmd.current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    match OsSession::spawn(cmd) {
        Ok(mut session) => {
            session.set_expect_timeout(Some(Duration::from_secs(10)));
            Some(session)
        }
        Err(e) => {
            eprintln!("pty_interactive: skipping — no PTY available: {e}");
            None
        }
    }
}

/// Sends raw bytes (text or control sequences) to the PTY.
fn send(session: &mut OsSession, bytes: &str) {
    session
        .send(bytes)
        .unwrap_or_else(|e| panic!("send {bytes:?} failed: {e}"));
}

/// Reads the PTY stream until `needle` appears, or panics on timeout.
/// `needle` is matched literally (not as a regex).
fn expect(session: &mut OsSession, needle: &str) {
    session
        .expect(needle)
        .unwrap_or_else(|e| panic!("expected {needle:?} but: {e}"));
}

/// Reads until the session ends (the child exited and the PTY closed).
fn expect_eof(session: &mut OsSession) {
    session
        .expect(Eof)
        .unwrap_or_else(|e| panic!("expected session EOF but: {e}"));
}

/// Builds a `(HISTFILE=...)` env pointing into `dir`, isolating
/// history per test.
#[allow(dead_code)]
fn histfile_env(dir: &Path) -> Vec<(&'static str, String)> {
    let hist = dir.join("huck_history");
    vec![("HISTFILE", hist.to_string_lossy().into_owned())]
}

/// Converts an owned-value env vec to the borrowed form `try_spawn`
/// expects.
#[allow(dead_code)]
fn env_refs<'a>(env: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    env.iter().map(|(k, v)| (*k, v.as_str())).collect()
}

#[test]
fn pty_huck_starts_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
    expect_eof(&mut session);
}
