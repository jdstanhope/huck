//! v132: `eval`/`source` must run their commands with the ENCLOSING StdoutSink
//! (capture + redirect). Pre-fix, an interactive `x=$(eval 'large-output-external')`
//! deadlocked: the captured external command re-entered job control and was never
//! handed the terminal, so the parent's wait hung. This is the nvm `ls-remote`
//! hang class. Skips (passes) if no PTY can be allocated.

use std::process::Command;
use std::time::Duration;

use expectrl::Expect;
use expectrl::session::OsSession;

#[test]
fn interactive_eval_capture_does_not_hang() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    // Hermetic: never source the developer's ~/.huckrc (#239).
    cmd.arg("--norc");
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("eval_source_hang_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(10)));

    let _ = session.send("echo WARM_$((6*7))");
    let _ = session.send("\r");
    assert!(
        session.expect("WARM_42").is_ok(),
        "shell did not start / read a command"
    );

    // Large captured external output through eval — pre-v132 this deadlocked.
    let _ = session.send("x=$(eval 'seq 1 500000'); echo \"EVLEN=${#x} EVDONE\"");
    let _ = session.send("\r");
    let ok = session.expect("EVDONE").is_ok();

    drop(session);
    assert!(
        ok,
        "interactive `x=$(eval 'seq 1 500000')` hung (v132): shell unresponsive after a large captured eval"
    );
}

#[test]
fn interactive_eval_pipe_capture_does_not_hang() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    // Hermetic: never source the developer's ~/.huckrc (#239).
    cmd.arg("--norc");
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("eval_source_hang_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(10)));

    let _ = session.send("echo WARM_$((6*7))");
    let _ = session.send("\r");
    assert!(
        session.expect("WARM_42").is_ok(),
        "shell did not start / read a command"
    );

    // A pipeline inside the captured eval — also exercised the job-control path.
    let _ = session.send("x=$(eval 'seq 1 200000 | wc -l'); echo \"WC=$x WCDONE\"");
    let _ = session.send("\r");
    let ok = session.expect("WCDONE").is_ok();

    drop(session);
    assert!(
        ok,
        "interactive `x=$(eval 'seq 1 200000 | wc -l')` hung (v132): shell unresponsive after a captured eval-pipe"
    );
}
