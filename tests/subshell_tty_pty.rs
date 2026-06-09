//! v124 Fix A: an interactive foreground subshell whose body is a pipeline with
//! large output must NOT deadlock. Pre-fix, `( command ls -1qA /usr/bin |
//! grep -q . )` hung: the subshell ran in a background process group that was
//! never handed the terminal, and the parent waited without WUNTRACED. This is
//! nvm's `nvm.sh:1485` form. Skips (passes) if no PTY can be allocated.

use std::process::Command;
use std::time::Duration;

use expectrl::session::OsSession;
use expectrl::Expect;

#[test]
fn interactive_subshell_pipeline_does_not_hang() {
    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("subshell_tty_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));

    let _ = session.send("echo WARM_$((6*7))");
    let _ = session.send("\r");
    assert!(session.expect("WARM_42").is_ok(), "shell did not start / read a command");

    let _ = session.send("( command ls -1qA /usr/bin | grep -q . ); echo SUB_$((7*8))");
    let _ = session.send("\r");
    let ok = session.expect("SUB_56").is_ok();

    drop(session);
    assert!(ok, "interactive subshell pipeline hung (Fix A): shell unresponsive after `( ls | grep -q . )`");
}
