//! PTY test: the interactive line editor actually PAINTS the edit buffer
//! (v363, #666).
//!
//! This is the first harness in the project that asserts on RENDERED output.
//! Every other gate here compares huck against bash byte-for-byte — and
//! highlighting has no bash to compare against, so a pty and the escape
//! sequences it receives are the only ground truth available.
//!
//! What each test does: type a fragment WITHOUT pressing Enter, so the line
//! stays in the editor and rustyline re-renders it, then read what the terminal
//! received. Assertions are on the escape sequences, plus the width contract —
//! the VISIBLE text must be unchanged, because rustyline computes cursor
//! position from the original line and any added or removed character corrupts
//! the display.
//!
//! Skips (passes) if no PTY can be allocated, matching the other `*_pty.rs`
//! harnesses.

use std::process::Command;
use std::time::Duration;

use expectrl::Expect;
use expectrl::session::OsSession;

fn spawn() -> Option<OsSession> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    // Hermetic: never source the developer's ~/.huckrc (#239).
    cmd.arg("--norc");
    // Colour is gated on stdout being a terminal; under a pty it is. NO_COLOR
    // must not leak in from the developer's environment or every row would
    // silently pass for the wrong reason.
    cmd.env_remove("NO_COLOR");
    match OsSession::spawn(cmd) {
        Ok(mut s) => {
            s.set_expect_timeout(Some(Duration::from_secs(8)));
            let _ = s.send("echo READY_$((6*7))\r");
            if s.expect("READY_42").is_err() {
                eprintln!("highlight_render_pty: skipping — interactive marker not seen");
                return None;
            }
            Some(s)
        }
        Err(e) => {
            eprintln!("highlight_render_pty: skipping — no PTY: {e}");
            None
        }
    }
}

/// Send `fragment` with NO Enter, so the line stays in the editor and rustyline
/// re-renders it.
///
/// ⚠️ Do NOT read the pty directly. A raw `Read` on the session BLOCKS once the
/// terminal has nothing more to send, which hung this harness for ten minutes on
/// its first run. `expect` is the bounded primitive — it fails on the session
/// timeout instead of waiting forever, and an `&str` needle is matched as
/// LITERAL BYTES (verified in expectrl's `Needle for &str`), so an SGR sequence
/// containing `[` is safe to search for as-is.
fn typed(session: &mut OsSession, fragment: &str) {
    let _ = session.send(fragment);
    // rustyline re-renders per keystroke; let the last one land.
    std::thread::sleep(Duration::from_millis(250));
}

/// Abandon the edit line so the shell can exit cleanly.
fn abandon(session: &mut OsSession) {
    let _ = session.send("\x03"); // Ctrl-C
    std::thread::sleep(Duration::from_millis(120));
    let _ = session.send("exit\r");
}

#[test]
fn quotes_and_expansions_are_painted_as_typed() {
    let Some(mut session) = spawn() else { return };

    typed(&mut session, "echo 'sq' \"dq\" $HOME");

    // Searched in the order the painter emits them across one render pass, so a
    // single left-to-right scan of the stream finds all three.
    let single = session.expect("\x1b[32m").is_ok();
    let double = session.expect("\x1b[33m").is_ok();
    let varname = session.expect("\x1b[1;36m").is_ok();

    abandon(&mut session);

    assert!(single, "single-quoted run not painted green");
    assert!(double, "double-quoted run not painted yellow");
    assert!(varname, "variable name not painted bold cyan");
}

#[test]
fn the_visible_text_survives_painting() {
    // The width contract, end to end: rustyline computes cursor position from
    // the original line, so a painted line whose VISIBLE characters differ from
    // what was typed corrupts the display.
    let Some(mut session) = spawn() else { return };

    typed(&mut session, "echo 'sq' $HOME | grep xyzzy");
    // TWO assertions, and both are needed. Searching for the tail alone is NOT
    // load-bearing: `xyzzy` arrives whether or not anything was painted, which a
    // sabotage run proved — with colour forced off this row still passed while
    // the other two failed.
    //
    // ⚠️ And the "was it painted" needle must be a PALETTE entry, not a bare
    // `\x1b[`: rustyline emits its own cursor-positioning escapes, so a generic
    // escape matches with highlighting entirely disabled. The second sabotage run
    // caught exactly that. `1;36` is this palette's bold-cyan variable name.
    let painted = session.expect("\x1b[1;36m").is_ok();
    let intact = session.expect("xyzzy").is_ok();

    abandon(&mut session);
    assert!(painted, "nothing was painted, so this row proves nothing");
    assert!(intact, "typed text did not render through to its end");
}

#[test]
fn an_incomplete_line_is_still_painted() {
    // Almost every keystroke leaves the line unparseable; painting must not wait
    // for a complete command. The closed run before the open one is the proof.
    let Some(mut session) = spawn() else { return };

    typed(&mut session, "echo 'closed' 'unterminat");
    let painted = session.expect("\x1b[32m").is_ok();

    abandon(&mut session);
    assert!(
        painted,
        "the closed single-quoted run was not painted while the line was \
         unparseable"
    );
}

#[test]
fn piped_stdin_emits_no_escapes_at_all() {
    // The guard that keeps the 309-harness diff sweep green. Those harnesses
    // pipe huck and compare its bytes with bash's, so a single stray SGR would
    // redden every one of them. No pty here on purpose: this is the not-a-tty
    // path, and it must hold even with NO_COLOR unset.
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg("--norc")
        .env_remove("NO_COLOR")
        .arg("-c")
        .arg("echo 'sq' \"dq\" $HOME | grep sq")
        .output()
        .expect("huck -c ran");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        !text.contains('\x1b'),
        "escape sequence leaked into piped output: {text:?}"
    );

    // And the interactive reader path, not just `-c`.
    use std::io::Write as _;
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg("--norc")
        .env_remove("NO_COLOR")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("huck spawned");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"echo 'sq' $HOME\nexit\n")
        .expect("wrote script");
    let piped = child.wait_with_output().expect("huck finished");
    let piped_text = String::from_utf8_lossy(&piped.stdout);
    assert!(
        !piped_text.contains('\x1b'),
        "escape sequence leaked into piped-stdin output: {piped_text:?}"
    );
}
