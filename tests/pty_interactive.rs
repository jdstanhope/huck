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

/// Builds an env with an isolated HISTFILE plus an empty PATH
/// directory, so command completion sees only builtins (deterministic).
fn isolated_env(dir: &Path) -> Vec<(&'static str, String)> {
    let hist = dir.join("huck_history");
    let empty_path = dir.join("emptybin");
    std::fs::create_dir_all(&empty_path).unwrap();
    vec![
        ("HISTFILE", hist.to_string_lossy().into_owned()),
        ("PATH", empty_path.to_string_lossy().into_owned()),
    ]
}

#[test]
fn tab_completes_builtin() {
    let dir = tempfile::tempdir().unwrap();
    let env = isolated_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "ec");
    send(&mut session, TAB);
    expect(&mut session, "echo");
    send(&mut session, ENTER);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn tab_double_tab_lists() {
    let dir = tempfile::tempdir().unwrap();
    let env = isolated_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, TAB);
    send(&mut session, TAB);
    expect(&mut session, "echo");
    expect(&mut session, "history");
    send(&mut session, CTRL_C);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn tab_completes_filename() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ptyfile_unique.txt"), b"").unwrap();
    let env = isolated_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo ptyfile_un");
    send(&mut session, TAB);
    expect(&mut session, "ptyfile_unique.txt");
    send(&mut session, ENTER);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn tab_completes_directory_slash() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("ptydir_unique")).unwrap();
    let env = isolated_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo ptydir_un");
    send(&mut session, TAB);
    expect(&mut session, "ptydir_unique/");
    send(&mut session, ENTER);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn tab_completes_variable() {
    let dir = tempfile::tempdir().unwrap();
    let hist = dir.path().join("huck_history");
    let env: Vec<(&str, &str)> = vec![
        ("HISTFILE", hist.to_str().unwrap()),
        ("HUCKPTYVAR", "ptyvarvalue"),
    ];
    let Some(mut session) = try_spawn(dir.path(), &env) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo $HUCKPTY");
    send(&mut session, TAB);
    send(&mut session, ENTER);
    expect(&mut session, "ptyvarvalue");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn up_arrow_recalls_previous() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo recallmarker");
    send(&mut session, ENTER);
    expect(&mut session, "recallmarker"); // sync past the command
    expect(&mut session, "huck> ");       // sync to the next prompt
    send(&mut session, UP);
    // If up-arrow recalled the entry, the line is redrawn as the full
    // previous command.
    expect(&mut session, "echo recallmarker");
    send(&mut session, ENTER);
    expect(&mut session, "recallmarker"); // it ran again
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn up_arrow_twice_recalls_older() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo olderone");
    send(&mut session, ENTER);
    expect(&mut session, "olderone");
    expect(&mut session, "huck> ");
    send(&mut session, "echo newertwo");
    send(&mut session, ENTER);
    expect(&mut session, "newertwo");
    expect(&mut session, "huck> ");
    send(&mut session, UP);
    send(&mut session, UP);
    expect(&mut session, "echo olderone");
    send(&mut session, ENTER);
    expect(&mut session, "olderone");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn down_arrow_navigates_forward() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo firstcmd");
    send(&mut session, ENTER);
    expect(&mut session, "firstcmd");
    expect(&mut session, "huck> ");
    send(&mut session, "echo secondcmd");
    send(&mut session, ENTER);
    expect(&mut session, "secondcmd");
    expect(&mut session, "huck> ");
    send(&mut session, UP);
    send(&mut session, UP);
    expect(&mut session, "echo firstcmd");
    send(&mut session, DOWN);
    expect(&mut session, "echo secondcmd");
    send(&mut session, ENTER);
    expect(&mut session, "secondcmd");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
