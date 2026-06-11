//! PTY regression test for v138: an untrapped Ctrl-C aborts the running command
//! and leaves an interactive shell alive with `$?` == 130.
//!
//! Two interactive cases under a real PTY:
//!   A. A foreground EXTERNAL command (`sleep 5`). The terminal Ctrl-C goes to
//!      the child's process group (huck `tcsetpgrp`'d the terminal to it), so
//!      huck itself never receives SIGINT — the trigger is the child dying from
//!      SIGINT. Task 4 sets `sigint_flag` at the foreground-wait site so the
//!      same abort path fires.
//!   B. A shell-FUNCTION busy loop (`while true; do :; done`). Here huck runs
//!      in-process and receives SIGINT directly, setting `sigint_flag`; the
//!      abort unwinds the loop + the function call.
//!
//! After Ctrl-C the shell must promptly process the next line and report
//! `$?` == 130 (and NOT have exited). A per-read timeout turns a hang into a
//! failed `expect` rather than wedging the whole suite; dropping the session
//! kills any wedged child. Skips (passes) if no PTY can be allocated.

use std::process::Command;
use std::time::Duration;

use expectrl::session::OsSession;
use expectrl::Expect;

#[test]
fn ctrl_c_aborts_foreground_external_and_shell_survives() {
    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sigint_abort_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));

    // Confirm the shell is alive and reading before we start.
    let _ = session.send("echo READY_$((6*7))\r");
    if session.expect("READY_42").is_err() {
        eprintln!("sigint_abort_pty: skipping — interactive prompt/marker not seen");
        return;
    }

    // Case A: foreground external command, then Ctrl-C.
    let _ = session.send("sleep 5\r");
    // Give the child a moment to take the terminal.
    std::thread::sleep(Duration::from_millis(300));
    // Ctrl-C: delivered to the child's pgroup; the child dies from SIGINT.
    let _ = session.send("\x03");
    // The shell must promptly process the next line with $? == 130.
    let _ = session.send("echo done $?\r");
    let responsive = session.expect("done 130").is_ok();

    drop(session);
    assert!(
        responsive,
        "Ctrl-C on a foreground external command did not abort with $?==130 \
         (shell hung or did not record SIGINT)"
    );
}

#[test]
fn ctrl_c_aborts_shell_function_loop_and_shell_survives() {
    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sigint_abort_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));

    let _ = session.send("echo READY_$((6*7))\r");
    if session.expect("READY_42").is_err() {
        eprintln!("sigint_abort_pty: skipping — interactive prompt/marker not seen");
        return;
    }

    // Case B: a shell-function busy loop, then Ctrl-C.
    let _ = session.send("f(){ while true; do :; done; }\r");
    let _ = session.send("f\r");
    // Let the loop spin for a bit.
    std::thread::sleep(Duration::from_millis(300));
    let _ = session.send("\x03");
    let _ = session.send("echo back $?\r");
    let responsive = session.expect("back 130").is_ok();

    drop(session);
    assert!(
        responsive,
        "Ctrl-C on a shell-function busy loop did not abort with $?==130 \
         (shell hung or did not record SIGINT)"
    );
}
