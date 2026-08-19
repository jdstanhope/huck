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
    // Hermetic: never source the developer's ~/.huckrc (#239). `--norc` takes
    // precedence over --rcfile/$HUCK_RC, so a future rc-file test must spawn
    // huck directly rather than through this helper.
    cmd.arg("--norc");
    // ⚠️ Highlighting OFF for this suite (#666). It is a REPL-BEHAVIOUR suite —
    // history recall, Ctrl-C, multi-line assembly, heredocs — and painting the
    // edit line multiplies its terminal traffic about fivefold, which on a
    // one-core box starved sessions until legitimate waits exceeded half a
    // minute. Measured: 110 s with two failures, 33 s and green without.
    //
    // The painted editor has its own harness, `highlight_render_pty.rs`. What
    // that does NOT cover is a CONTINUATION line, and the reason is worth
    // knowing: a continuation line is parsed on its own, so `then echo hi` has
    // no `if` in front of it, the parse fails at the first word, and almost
    // nothing gets marked. Highlighting the accumulated command rather than the
    // physical line is #670 — so turning colour off here costs no coverage that
    // exists today.
    cmd.env("NO_COLOR", "1");
    cmd.current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    match OsSession::spawn(cmd) {
        Ok(mut session) => {
            // 30 s, not 10. Since v363 the editor repaints the WHOLE line on
            // every keystroke, so each session does several times the terminal
            // I/O it used to. Individually every test here still finishes in a
            // few seconds — `pty_multiline_if_runs` alone takes 4 s — but 26
            // sequential pty sessions on a 1-core box pushed two or three of
            // them past a 10 s expect, and WHICH ones moved between runs, which
            // is the signature of contention rather than a broken expectation.
            //
            // Raising the ceiling rather than the sleeps on purpose: a timeout
            // that is never reached costs nothing, while longer sleeps would slow
            // every run whether or not the box is loaded.
            //
            // 90 s, which is headroom for a loaded box rather than for any
            // expected wait: this repo has 124 integration binaries and one
            // core, so a legitimate wait here has been measured past 30 s.
            // Nothing waits 90 s when it passes.
            session.set_expect_timeout(Some(Duration::from_secs(90)));
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
///
/// ⚠️ Choose needles that can only appear ONCE, at the point you mean. Since
/// v363 the editor repaints prompt-and-line on every keystroke, so a prompt is
/// emitted many times per line and `"> "` — which `"huck> "` also contains — no
/// longer identifies a position. Prefer a marker that only the command's OUTPUT
/// can produce, e.g. `echo TAG_$((6*7))` asserted as `TAG_42`: the typed line
/// echoes the expression, only the output holds the value.
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
    assert!(
        listing.contains("echo"),
        "double-tab listing missing 'echo': {listing:?}"
    );
    assert!(
        listing.contains("history"),
        "double-tab listing missing 'history': {listing:?}"
    );
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
    expect(&mut session, "huck> "); // sync to the next prompt
    send(&mut session, UP);
    // If up-arrow recalled the entry, the line is redrawn with the previous
    // command in it.
    //
    // ⚠️ The ARGUMENT is the needle, not the whole line: since #666 the command
    // word is painted, so `echo recallmarker` is written to the terminal as
    // `<SGR>echo<SGR> recallmarker` and no literal needle spans the two. The
    // argument is unpainted, and this is read forward from the prompt we just
    // synced on, so the next occurrence IS the redraw.
    expect(&mut session, "recallmarker");
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
    // The unpainted ARGUMENT is the needle — see `up_arrow_recalls_previous`.
    expect(&mut session, "olderone");
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
    // Unpainted ARGUMENTS are the needles — see `up_arrow_recalls_previous`.
    expect(&mut session, "firstcmd");
    send(&mut session, DOWN);
    expect(&mut session, "secondcmd");
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
    // After Ctrl-C rustyline discards the partial line and the loop redraws a
    // fresh prompt. Wait for the redraw before typing `pwd`, so the keystrokes
    // are not sent into the editor mid-redraw.
    //
    // ⚠️ `expect("huck> ")` is NOT the way to wait for it. Every keystroke
    // repaints prompt-and-line, so by this point the stream holds one prompt per
    // character typed; the expect matches an EARLIER redraw, returns immediately,
    // and the `pwd` goes out during the real redraw. `settle()` is the sync for a
    // boundary with no unique output of its own (see its boundary #2).
    settle();
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
    // The continuation prompt appears once here, BEFORE any body is typed, so
    // this sync is unambiguous.
    expect(&mut session, "> ");
    // ⚠️ The marker is written as an EXPANSION so the string that appears in the
    // command's OUTPUT (`MARKER_42`) never appears in the ECHO of the typed line
    // (`MARKER_$((6*7))`). Matching on the echo would pass without the `if` ever
    // running. The repo's own `READY_$((6*7))` spawn probe uses the same trick.
    // ⚠️ ONE send, and the sync below is the ONLY read of `IFSYNC`. Splitting
    // this into two chunks with a read between them — which an earlier version
    // did, to drain the pty — silently broke the test: the typed line is echoed
    // ONCE, so the intermediate read consumed the very occurrence the sync after
    // `ENTER` was waiting for, and that sync then waited out the full ceiling.
    send(&mut session, "then echo IFSYNC MARKER_$((6*7))");
    send(&mut session, ENTER);
    // ⚠️ DRAIN, not just sleep. Since v363 the editor repaints the whole line on
    // every keystroke, so a session emits several times the bytes it used to. A
    // test that only reads inside `expect` leaves that output in the pty buffer;
    // once it fills, HUCK BLOCKS ON WRITE and stops consuming input, so the `if`
    // never completes and the marker never arrives. That is what made this test
    // fail intermittently — and which test failed moved between runs.
    //
    // Expecting the echo of a PLAIN word drains the buffer again and syncs.
    //
    // The sync word must contain no shell metacharacter: an expansion in it would
    // be painted, and the escapes interleaved into the echo would break a
    // literal match.
    expect(&mut session, "IFSYNC");
    send(&mut session, "fi");
    send(&mut session, ENTER);
    // The body runs only if the three lines were assembled into one
    // complete `if` command — and only the OUTPUT can contain this.
    expect(&mut session, "MARKER_42");
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
    // ⚠️ No `expect("> ")` here, and no `expect("huck> ")` after the abort.
    // Both are AMBIGUOUS sync points: `"huck> "` itself contains `"> "`, and
    // since v363 the editor repaints prompt-and-line on every keystroke, so each
    // typed character emits another copy of the prompt. An intermediate expect
    // then matches an arbitrary earlier repaint, the stream position drifts, and
    // the final assertion times out looking for output that has already gone
    // past. This test failed exactly that way in CI.
    //
    // The two remaining expects are unambiguous: the FIRST prompt, and a marker
    // that can only come from the command's output. Continuation-prompt coverage
    // is not lost — `pty_continuation_prompt_appears` asserts it directly.
    settle();
    send(&mut session, CTRL_C);
    settle();
    // After the abort the partial command is gone, so a fresh command runs alone.
    // `pwd` proves the cwd; the arithmetic marker proves the OUTPUT was reached
    // (it echoes as `CTRLC_$((6*7))` when typed and prints `CTRLC_42`, so a pass
    // cannot be matching the echo of the line we just typed).
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    send(&mut session, "pwd; echo CTRLC_$((6*7))");
    send(&mut session, ENTER);
    expect(&mut session, marker);
    expect(&mut session, "CTRLC_42");
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
    // Unambiguous: the continuation prompt is drawn once before any body typing.
    expect(&mut session, "> ");
    // ⚠️ An EXPANSION in the body, so what `cat` prints (`HEREDOC_42`) differs
    // from the echo of the typed line (`HEREDOC_$((6*7))`). An unquoted heredoc
    // delimiter expands the body, so this also proves the body took the
    // expanding path rather than being copied verbatim.
    send(&mut session, "HDSYNC HEREDOC_$((6*7))");
    send(&mut session, ENTER);
    // Drain + sync on a plain word — see `pty_multiline_if_runs` for why a sleep
    // is not enough and why the word must have no metacharacters in it.
    expect(&mut session, "HDSYNC");
    send(&mut session, "EOF");
    send(&mut session, ENTER);
    // `cat` echoes the body; the prompt must return afterwards.
    expect(&mut session, "HEREDOC_42");
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
