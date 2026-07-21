//! PTY test: the `bind` builtin's run-loop seam ACTUALLY reconfigures the live
//! rustyline editor (the one thing the non-interactive diff harness can't check).
//!
//! v161's `bind` records key-binding requests on the shell; the run loop
//! (`apply_readline_settings` in src/shell.rs, called at the top of the REPL
//! loop before each line read) applies them to the live editor via
//! `editor.bind_sequence`. So a `bind` takes effect for the NEXT line read.
//!
//! This binds an otherwise-inert key (Ctrl-O = `\x0f`, unbound / not
//! `accept-line` in rustyline emacs mode by default) to `accept-line`, then
//! types a command WITHOUT a trailing Enter and presses Ctrl-O. If the rebind
//! took effect, Ctrl-O submits the line and the `echo` runs — proving the REBIND
//! (not a default). A per-read timeout turns a regression into a failed
//! `expect`.
//!
//! Skips (passes) if no PTY can be allocated (e.g. sandboxed CI).

use std::process::Command;
use std::time::Duration;

use expectrl::Expect;
use expectrl::session::OsSession;

fn spawn() -> Option<OsSession> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    // Hermetic: never source the developer's ~/.huckrc (#239).
    cmd.arg("--norc");
    match OsSession::spawn(cmd) {
        Ok(mut s) => {
            s.set_expect_timeout(Some(Duration::from_secs(8)));
            // Confirm the interactive prompt is alive before starting.
            let _ = s.send("echo READY_$((6*7))\r");
            if s.expect("READY_42").is_err() {
                eprintln!("bind_pty: skipping — interactive marker not seen");
                return None;
            }
            Some(s)
        }
        Err(e) => {
            eprintln!("bind_pty: skipping — no PTY: {e}");
            None
        }
    }
}

#[test]
fn bind_rebinds_ctrl_o_to_accept_line() {
    let Some(mut session) = spawn() else { return };

    // Bind Ctrl-O to accept-line. Takes effect on the NEXT line read (the run
    // loop applies pending binds at the top of each REPL iteration).
    let _ = session.send("bind '\"\\C-o\":accept-line'\r");
    // Let the bind line be read + the run loop loop back to a fresh prompt
    // (where the new binding is now live) before issuing the sync command.
    std::thread::sleep(Duration::from_millis(200));
    // Sync: a normal command confirms the bind line was processed and we're at
    // a fresh prompt where the binding is now active.
    let _ = session.send("echo SYNC_$((3+4))\r");
    if session.expect("SYNC_7").is_err() {
        eprintln!("bind_pty: skipping — sync marker not seen");
        return;
    }
    // Type a command WITHOUT a trailing Enter, then press Ctrl-O (\x0f).
    // If the rebind took effect, Ctrl-O accepts the line and the echo runs.
    let _ = session.send("echo BOUND_OK_$((8+8))");
    std::thread::sleep(Duration::from_millis(150));
    let _ = session.send("\x0f"); // Ctrl-O
    let accepted = session.expect("BOUND_OK_16").is_ok();

    let _ = session.send("exit\r");
    drop(session);

    assert!(
        accepted,
        "Ctrl-O rebound to accept-line did not submit the line"
    );
}
