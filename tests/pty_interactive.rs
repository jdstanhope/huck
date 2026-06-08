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

/// Brief pause to let huck cross a terminal-mode boundary that produces
/// no output to sync on. Two boundaries need it:
///
/// 1. **Before sending Ctrl-C/Ctrl-Z into a blocking builtin/pipeline:**
///    rustyline echoes the submitted line *before* huck leaves raw mode
///    and enters the builtin, so a control char sent the instant the echo
///    is seen can land in raw mode (where `\x03` is a line-edit key, not a
///    signal). Pausing guarantees huck reached the cooked-mode poll loop.
///
/// 2. **After a control-char-induced transition, before sending the next
///    command:** when huck returns to the REPL after Ctrl-C/Ctrl-Z (job
///    stopped, `wait` interrupted, heredoc/continuation aborted), it
///    redraws the prompt and rustyline RE-ENTERS raw mode, which flushes
///    pending terminal input (`TCSAFLUSH`). The redrawn `huck> ` prompt is
///    therefore necessary but NOT sufficient: a command sent in the window
///    between the prompt appearing and rustyline's read being ready is
///    silently discarded, after which huck waits forever and the next
///    `expect()` times out. Under CPU load (the 23 pty tests run in
///    parallel) this window widens enough to drop the keystrokes. Pausing
///    before the post-transition send lets rustyline finish re-entry first.
///    (This is a test-synchronization concern, not a huck bug — a real
///    user typing after the visible prompt is far slower than this window.)
fn settle() {
    std::thread::sleep(Duration::from_millis(600));
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
    // The double-tab lists builtins in a multi-column (column-major) layout, so
    // the stream order of any two entries depends on column packing and reflows
    // as builtins are added. Capture the whole listing up to the redrawn prompt
    // and assert membership, rather than expecting two entries in a fixed order
    // (which broke when `getopts` was added in v111 and shifted the columns).
    let caps = session
        .expect("huck> ")
        .unwrap_or_else(|e| panic!("no prompt redraw after double-tab: {e}"));
    let listing = String::from_utf8_lossy(caps.before());
    assert!(listing.contains("echo"), "double-tab listing missing 'echo': {listing:?}");
    assert!(listing.contains("history"), "double-tab listing missing 'history': {listing:?}");
    send(&mut session, CTRL_C);
    expect(&mut session, "huck> ");
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

#[test]
fn ctrl_c_empty_prompt_survives() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, CTRL_C);
    // After Ctrl-C rustyline aborts the line and the loop redraws a
    // fresh prompt. Sync to it before typing so the keystrokes are not
    // sent into the editor mid-redraw.
    expect(&mut session, "huck> ");
    // The shell must still be alive: a command sent afterwards runs.
    send(&mut session, "echo aftersigint");
    send(&mut session, ENTER);
    expect(&mut session, "aftersigint");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn ctrl_c_clears_partial_line() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Type a partial line with NO Enter, then Ctrl-C.
    send(&mut session, "echo partialXYZ");
    send(&mut session, CTRL_C);
    // After Ctrl-C rustyline discards the partial line and the loop
    // redraws a fresh prompt. Sync to it before typing `pwd` so the
    // keystrokes are not sent into the editor mid-redraw.
    expect(&mut session, "huck> ");
    // Run `pwd`. If Ctrl-C cleared the partial line, `pwd` runs alone
    // and prints the cwd. If it did NOT clear, the line would be
    // `echo partialXYZpwd` and the cwd path would never be printed.
    send(&mut session, "pwd");
    send(&mut session, ENTER);
    // The temp dir's unique random component appears only if `pwd`
    // ran clean — it is never part of the typed input.
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    expect(&mut session, marker);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn ctrl_c_breaks_out_of_wait() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Background a long sleep so `wait` blocks.
    send(&mut session, "sleep 30 &");
    send(&mut session, ENTER);
    expect(&mut session, "[1]"); // background job notification
    expect(&mut session, "huck> ");
    send(&mut session, "wait");
    send(&mut session, ENTER);
    // Sync past the echoed `wait` line, then settle: rustyline echoes
    // the line *before* huck enters `wait`'s cooked-mode poll loop, so
    // Ctrl-C sent the instant the echo is seen could be eaten by the
    // editor in raw mode. The pause guarantees `wait` is blocking.
    expect(&mut session, "wait");
    settle();
    // Ctrl-C must break the blocking `wait` and return to the prompt.
    send(&mut session, CTRL_C);
    // Sync to the fresh prompt the loop redraws after `wait` returns, then
    // settle: the prompt alone is not enough — rustyline's raw-mode re-entry
    // flushes type-ahead, so `echo afterwait` typed in that window is lost
    // under load (settle()'s boundary #2).
    expect(&mut session, "huck> ");
    settle();
    send(&mut session, "echo afterwait");
    send(&mut session, ENTER);
    expect(&mut session, "afterwait");
    send(&mut session, "exit");
    send(&mut session, ENTER);
    // The orphaned `sleep 30` is reparented to init and exits on its
    // own — harmless.
}

#[test]
fn ctrl_d_empty_prompt_exits() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Ctrl-D (EOF) at an empty prompt exits the shell.
    send(&mut session, CTRL_D);
    expect_eof(&mut session);
}

#[test]
fn pty_continuation_prompt_appears() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // An unterminated `if` must draw the `> ` continuation prompt.
    send(&mut session, "if true");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_multiline_if_runs() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "if true");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "then echo MARKER42");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "fi");
    send(&mut session, ENTER);
    // The body runs only if the three lines were assembled into one
    // complete `if` command.
    expect(&mut session, "MARKER42");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_ctrl_c_aborts_multiline_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Start a multi-line `if`, then abort it with Ctrl-C.
    send(&mut session, "if true");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    settle();
    send(&mut session, CTRL_C);
    // After the abort the main prompt returns and the partial command
    // is gone — a fresh `pwd` runs alone and prints the temp dir name.
    expect(&mut session, "huck> ");
    send(&mut session, "pwd");
    send(&mut session, ENTER);
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    expect(&mut session, marker);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_multi_stage_pipeline_completes_via_pgrp_wait() {
    // After B-09, run_multi_stage's interactive path waits on the whole
    // process group via waitpid(-pgid, …, WUNTRACED). This test exercises
    // that path with a 3-stage pipeline and verifies the data flows through
    // and the prompt returns.
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "echo PIPE_MARKER | cat | cat");
    send(&mut session, ENTER);
    expect(&mut session, "PIPE_MARKER");
    // Subsequent prompt confirms the wait loop returned cleanly (no wedge).
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_heredoc_simple() {
    // Type a complete heredoc interactively: the body line is echoed back
    // by `cat` and the main prompt returns.
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "cat <<EOF");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "PTY_HEREDOC_MARKER");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "EOF");
    send(&mut session, ENTER);
    // `cat` echoes the body; the prompt must return afterwards.
    expect(&mut session, "PTY_HEREDOC_MARKER");
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_heredoc_continuation_prompt_appears() {
    // After `cat <<EOF<ENTER>`, the REPL should draw the `> ` continuation
    // prompt while waiting for heredoc body lines.  Ctrl-C aborts and
    // returns the main prompt.
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "cat <<EOF");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    // Abort the heredoc body collection.
    settle();
    send(&mut session, CTRL_C);
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_compound_stage_pipeline_stops_and_resumes() {
    // Start `cat | if true; then sleep 5; fi` — both stages are in the
    // pipeline's process group.  Ctrl-Z (SIGTSTP) stops the whole group.
    // We expect a "Stopped" notification and the prompt to return.  Then we
    // kill the job so the test doesn't hang waiting for sleep to finish.
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "cat | if true; then sleep 5; fi");
    send(&mut session, ENTER);
    // Give the pipeline time to start and reach the blocking sleep.
    settle();
    // Ctrl-Z stops both stages (the whole pgrp via SIGTSTP).
    send(&mut session, "\x1a");
    expect(&mut session, "Stopped");
    expect(&mut session, "huck> ");
    // Let rustyline finish re-entering raw mode after the stop before we
    // type again — see settle()'s boundary #2 (the redrawn prompt alone is
    // not a safe barrier; the keystrokes can be flushed under load).
    settle();
    // Kill the stopped job so the test exits cleanly.
    send(&mut session, "kill %1");
    send(&mut session, ENTER);
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_heredoc_ctrl_c_aborts_body_collection() {
    // Start a heredoc, type a partial body line, then abort with Ctrl-C.
    // The partial command must be discarded; a subsequent `pwd` must run
    // cleanly and print the temp-dir path.
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "cat <<EOF");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "partial body");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    settle();
    send(&mut session, CTRL_C);
    // Buffer was discarded — confirm by running a fresh command.
    expect(&mut session, "huck> ");
    settle(); // post-transition raw-mode re-entry flushes type-ahead (boundary #2)
    send(&mut session, "pwd");
    send(&mut session, ENTER);
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    expect(&mut session, marker);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_subshell_continuation_prompt_appears() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "(echo hi");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, ")");
    send(&mut session, ENTER);
    expect(&mut session, "hi");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_subshell_ctrl_c_aborts_body_collection() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "(echo hi");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    settle();
    send(&mut session, CTRL_C);
    expect(&mut session, "huck> ");
    // Buffer was discarded — confirm by running a fresh command.
    settle(); // post-transition raw-mode re-entry flushes type-ahead (boundary #2)
    send(&mut session, "pwd");
    send(&mut session, ENTER);
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    expect(&mut session, marker);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

// ── v81 select interactive pick tests ───────────────────────────────────────

/// Send a `select` loop interactively; pick item 2 and verify the body runs.
#[test]
fn pty_select_pick() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(
        &mut session,
        "select x in alpha beta gamma; do echo \"got=$x reply=$REPLY\"; break; done",
    );
    send(&mut session, ENTER);
    // After the command is submitted the menu appears on stderr (mixed into
    // the pty stream).  Wait for each item so we know the menu was printed.
    expect(&mut session, "1) alpha");
    expect(&mut session, "2) beta");
    expect(&mut session, "3) gamma");
    expect(&mut session, "#? ");
    // Settle: `select`'s `read` re-enters raw mode on the pty after printing
    // the prompt; the prompt alone is not a sufficient readiness barrier (same
    // TCSAFLUSH race as documented in settle()).
    settle();
    // Pick item 2.
    send(&mut session, "2");
    send(&mut session, ENTER);
    // Body should echo the item and REPLY, then break returns the prompt.
    expect(&mut session, "got=beta reply=2");
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

/// Invalid index (out of range) — body runs with NAME empty, REPLY set to input.
#[test]
fn pty_select_invalid_index_runs_body() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(
        &mut session,
        r#"select x in a b; do echo "x=[$x] r=$REPLY"; break; done"#,
    );
    send(&mut session, ENTER);
    expect(&mut session, "1) a");
    expect(&mut session, "2) b");
    expect(&mut session, "#? ");
    settle();
    // Send an out-of-range index (9 > 2 items).
    send(&mut session, "9");
    send(&mut session, ENTER);
    // NAME should be empty, REPLY should be "9".
    expect(&mut session, "x=[] r=9");
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

/// Empty line (just ENTER) reprints the menu without running the body;
/// a subsequent valid pick does run the body.
#[test]
fn pty_select_empty_line_reprints_menu() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(
        &mut session,
        r#"select x in a b; do echo "got=$x"; break; done"#,
    );
    send(&mut session, ENTER);
    // First menu print.
    expect(&mut session, "1) a");
    expect(&mut session, "#? ");
    settle();
    // Send empty line — menu should reprint.
    send(&mut session, ENTER);
    // Expect the SECOND menu print (a new "1) a" after the empty-line reprint).
    expect(&mut session, "1) a");
    expect(&mut session, "#? ");
    settle();
    // Now pick item 1.
    send(&mut session, "1");
    send(&mut session, ENTER);
    expect(&mut session, "got=a");
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
