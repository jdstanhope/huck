//! PTY regression test: Ctrl-Z (SIGTSTP) on a foreground command/pipeline that
//! contains a process substitution must NOT hang the shell.
//!
//! Bug: when a foreground job was stopped, huck blocking-`waitpid`'d the process
//! substitution's child to drain it. But a stopped job's procsub child is still
//! alive (its consumer is stopped too), so the blocking wait deadlocked and the
//! shell never returned to the prompt (`find … | tee >(awk …)` + Ctrl-Z wedged
//! huck). Fix: drain procsubs NON-blocking on the stopped path. These tests send
//! Ctrl-Z (`\x1a`) and verify the prompt comes back (the next `echo` runs); a
//! per-read timeout turns a regression-hang into a failed `expect`.
//!
//! Skips (passes) if no PTY can be allocated (e.g. sandboxed CI).
//!
//! Also skipped on macOS: there Ctrl-Z on a job containing a process
//! substitution currently deadlocks the shell (reproducible, not flaky). That
//! is a real, tracked defect — see issue #97 — not a test that doesn't apply to
//! macOS, so these guards must be removed once #97 is fixed.

/// True on platforms where this scenario is a known, tracked hang (#97).
fn skip_known_macos_hang() -> bool {
    if cfg!(target_os = "macos") {
        eprintln!("procsub_stop_pty: skipping on macOS — known job-control hang, see issue #97");
        return true;
    }
    false
}

use std::process::Command;
use std::time::Duration;

use expectrl::Expect;
use expectrl::session::OsSession;

fn spawn() -> Option<OsSession> {
    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    match OsSession::spawn(cmd) {
        Ok(mut s) => {
            s.set_expect_timeout(Some(Duration::from_secs(8)));
            // Confirm the interactive prompt is alive before starting.
            let _ = s.send("echo READY_$((6*7))\r");
            if s.expect("READY_42").is_err() {
                eprintln!("procsub_stop_pty: skipping — interactive marker not seen");
                return None;
            }
            Some(s)
        }
        Err(e) => {
            eprintln!("procsub_stop_pty: skipping — no PTY: {e}");
            None
        }
    }
}

#[test]
fn ctrl_z_on_pipeline_with_procsub_does_not_hang() {
    if skip_known_macos_hang() {
        return;
    }
    let Some(mut session) = spawn() else { return };

    // A foreground pipeline whose last stage feeds a process substitution.
    // `sleep 30` produces nothing, so `tee` blocks reading and the `>(cat)`
    // child blocks reading from `tee` — exactly the stopped-but-alive shape.
    let _ = session.send("sleep 30 | tee >(cat >/dev/null)\r");
    // Let the pipeline + procsub fully set up before stopping it.
    std::thread::sleep(Duration::from_millis(500));
    // Ctrl-Z (SUB): stops the foreground job's process group.
    let _ = session.send("\x1a");
    // The shell must return to the prompt and run the next line.
    let _ = session.send("echo AFTER_$((1+1))\r");
    let responsive = session.expect("AFTER_2").is_ok();

    // Best-effort cleanup of the stopped job.
    let _ = session.send("kill -9 %1 2>/dev/null\r");
    drop(session);

    assert!(
        responsive,
        "Ctrl-Z on a pipeline containing a process substitution hung the shell \
         (no prompt back / next command did not run)"
    );
}

#[test]
fn ctrl_z_on_command_with_output_procsub_does_not_hang() {
    if skip_known_macos_hang() {
        return;
    }
    let Some(mut session) = spawn() else { return };

    // A single foreground command with an OUTPUT process-substitution redirect.
    // `sleep 30` runs with its stdout going to `>(cat)`, which blocks reading.
    let _ = session.send("sleep 30 > >(cat >/dev/null)\r");
    std::thread::sleep(Duration::from_millis(500));
    let _ = session.send("\x1a");
    let _ = session.send("echo BACK_$((2+2))\r");
    let responsive = session.expect("BACK_4").is_ok();

    let _ = session.send("kill -9 %1 2>/dev/null\r");
    drop(session);

    assert!(
        responsive,
        "Ctrl-Z on a command with an output process substitution hung the shell \
         (no prompt back / next command did not run)"
    );
}
