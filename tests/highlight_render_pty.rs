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
    spawn_with(&[])
}

/// `spawn`, plus environment overrides — for the rows that prove a gate turns
/// colour OFF, which need to change the environment huck starts in.
fn spawn_with(env: &[(&str, &str)]) -> Option<OsSession> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    // Hermetic: never source the developer's ~/.huckrc (#239).
    cmd.arg("--norc");
    // Colour is gated on stdout being a terminal; under a pty it is. NO_COLOR
    // must not leak in from the developer's environment or every row would
    // silently pass for the wrong reason.
    cmd.env_remove("NO_COLOR");
    for (k, v) in env {
        cmd.env(k, v);
    }
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

#[test]
fn a_command_that_does_not_exist_is_painted_red() {
    // The design's one loud signal, end to end (#666, Task 4).
    //
    // ⚠️ Only the POSITIVE half is assertable here. The stream carries every
    // intermediate render, and while `echo` is being typed `e`, `ec` and `ech`
    // are each genuinely not commands — so red appears on the way to a perfectly
    // valid line. That is the intended behaviour (fish does the same), not
    // something to assert against. "A command that resolves is left alone" is
    // pinned on the finished line instead, in `completion_helper`'s unit tests,
    // where the final state can actually be inspected.
    let Some(mut session) = spawn() else { return };

    typed(&mut session, "nosuchcmd_xyz");
    let red = session.expect("\x1b[31m").is_ok();

    abandon(&mut session);

    assert!(red, "a command that does not resolve must be painted red");
}

#[test]
fn moving_the_cursor_onto_a_bracket_re_renders_it_emphasised() {
    // The one part of highlighting that is NOT a function of the text, so it is
    // the one part a unit test cannot reach: rustyline has to ASK for a repaint
    // when only the cursor moved (#666, Task 6).
    //
    // ⚠️ The obvious version of this test is not load-bearing, and a sabotage run
    // proved it: with the `MoveCursor` repaint switched off, rustyline redraws
    // the line from its CACHED painted string, so the emphasis computed while
    // typing is still in the stream and any "did reverse video appear" needle
    // matches. The needle has to be one that the cached string CANNOT contain.
    //
    // So the emphasis is moved to a DIFFERENT pair than the one it was last
    // computed for, and the needle names which:
    //
    //   1. type `echo $(date)` — the cursor lands just past `)`, so the `$(`
    //      pair is emphasised and `<reverse>$` appears once;
    //   2. type ` "x"` — the cursor is now on the QUOTE pair, so every later
    //      render emphasises `"`, never `$`;
    //   3. Ctrl-A, then five RIGHTs, puts the cursor on the `$` at offset 5.
    //      A stale cached render still says `"`; only a fresh parse says `$`.
    let Some(mut session) = spawn() else { return };

    typed(&mut session, "echo $(date)");
    let while_typing = session.expect("\x1b[7m$").is_ok();

    typed(&mut session, " \"x\"");
    typed(&mut session, "\x01"); // Ctrl-A — beginning of line
    for _ in 0..5 {
        let _ = session.send("\x1b[C"); // RIGHT
    }
    std::thread::sleep(Duration::from_millis(250));
    let after_moving = session.expect("\x1b[7m$").is_ok();

    abandon(&mut session);

    assert!(while_typing, "closing the pair did not emphasise it");
    assert!(
        after_moving,
        "moving the cursor onto the `$(` did not re-render its pair"
    );
}

#[test]
fn a_whole_delimiter_is_emphasised_end_to_end() {
    // ⚠️ Reported from USING the shell, which is why this row exists at the pty
    // layer too: emphasising a single character lit the `$` of `$(` and left the
    // bracket plain, and `${x}` lit the `x`. The needle is the WHOLE opener with
    // reverse video in front of it, so a one-character emphasis cannot match it.
    let Some(mut session) = spawn() else { return };

    typed(&mut session, "echo ${x}");
    let braced = session.expect("\x1b[7m${").is_ok();

    abandon(&mut session);

    assert!(
        braced,
        "the brace opener was not emphasised as one delimiter"
    );
}

/// Type a command that would be painted RED (it does not resolve), and report
/// whether the red actually reached the terminal.
///
/// ⚠️ ONE needle, and a palette entry rather than a bare `\x1b[`: rustyline emits
/// cursor-positioning escapes on every render, so a generic escape is present
/// whether or not anything was painted — a sabotage run proved that the hard
/// way. One needle also matters for cost: a negative answer waits out the
/// session timeout, so a list of five needles is five timeouts.
fn painted_red(session: &mut OsSession) -> bool {
    typed(session, "nosuchcmd_xyz");
    let red = session.expect("\x1b[31m").is_ok();
    // Then RUN it. The command fails harmlessly, and its error line is a unique
    // point to sync on — which is what a caller needs before doing anything
    // else, since a prompt is redrawn by every keystroke and identifies nothing.
    // Syncing on real output beats sleeping: a fixed sleep that is long enough
    // on an idle box is not long enough on a loaded one, which is exactly how
    // this test failed at two threads while passing at one.
    let _ = session.send("\r");
    assert!(
        session.expect("nosuchcmd_xyz: command not found").is_ok(),
        "the line never ran, so the session position is unknown from here on"
    );
    settle();
    red
}

/// Pause after a submitted line, before sending the next one.
///
/// ⚠️ Necessary, and seeing the command's OUTPUT is NOT a substitute. When huck
/// returns to the prompt, rustyline re-enters raw mode with `TCSAFLUSH`, which
/// DISCARDS terminal input that arrived in the meantime — so a line sent the
/// instant the previous command's output appears can vanish without trace.
/// Measured while writing this file: three commands piped into a pty at once ran
/// the FIRST and silently dropped the other two. `pty_interactive.rs` documents
/// the same boundary.
fn settle() {
    std::thread::sleep(Duration::from_millis(250));
}

#[test]
fn no_color_in_the_environment_disables_painting() {
    // The convention every tool that colours output is expected to honour: any
    // value at all means "do not".
    let Some(mut session) = spawn_with(&[("NO_COLOR", "1")]) else {
        return;
    };
    let painted = painted_red(&mut session);
    abandon(&mut session);
    assert!(!painted, "NO_COLOR did not disable painting");
}

#[test]
fn shopt_u_syntax_highlight_disables_painting() {
    // The control that composes with an rc file, and the one a user reaches for.
    // Read LIVE rather than resolved at startup, so it takes effect on the very
    // next keystroke — which is what this row proves: the option is turned off
    // from the prompt, in the session already running.
    let Some(mut session) = spawn() else { return };

    // With it on the same fragment paints, or the assertion below would pass for
    // the wrong reason.
    let before = painted_red(&mut session);
    let _ = session.send("shopt -u syntax_highlight\r");
    // ⚠️ A settle, not a sync: `shopt -u` prints NOTHING, so there is no output
    // to wait for — and sending the next line immediately loses it to the
    // `TCSAFLUSH` described on `settle`.
    settle();
    // Prove the option actually changed in the LIVE shell before concluding
    // anything from the absence of colour — otherwise a typo in the option name
    // would look exactly like a working gate.
    let _ = session.send("shopt syntax_highlight\r");
    assert!(
        session.expect("syntax_highlight\toff").is_ok(),
        "the option did not turn off in the running shell"
    );
    settle();

    let after = painted_red(&mut session);
    abandon(&mut session);

    assert!(before, "nothing was painted even with the option ON");
    assert!(!after, "shopt -u syntax_highlight did not disable painting");
}

#[test]
fn a_continuation_line_is_painted_like_the_command_it_continues() {
    // #670: a continuation line used to arrive plain. It is parsed ALONE, so
    // `then nosuchcmd_xyz` fails at its first word and nothing after it is even
    // scanned — there was nothing to colour. The editor now parses the
    // accumulated command and paints only the visible line of it.
    //
    // The needle is the KEYWORD colour: `then` is a keyword only in the company
    // of the `if` above it, so nothing but the fix can produce it here. Bold
    // blue never appears while typing `if true` (a bare `if` is a keyword, so
    // the first line is drained past before this is read).
    let Some(mut session) = spawn() else { return };

    let _ = session.send("if true\r");
    settle();
    // Sync past the first line's own renders, so what follows can only come
    // from the continuation line.
    assert!(
        session.expect("\x1b[1;34mif").is_ok(),
        "the first line was not painted, so this proves nothing about the second"
    );

    typed(&mut session, "then nosuchcmd_xyz");
    let keyword = session.expect("\x1b[1;34mthen").is_ok();
    let invalid = session.expect("\x1b[31mnosuchcmd_xyz").is_ok();

    abandon(&mut session);

    assert!(keyword, "`then` was not painted as a keyword");
    assert!(
        invalid,
        "the invalid command on the continuation line was not painted"
    );
}
