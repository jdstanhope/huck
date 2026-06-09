//! PTY regression test for M-116: interactive TAB completion must NOT hang when
//! a completion function runs an external command/pipeline. bash-completion's
//! `_longopt` (bound to `ls`, `cat`, …) runs
//! `$(compgen -W "$(LC_ALL=C $1 --help 2>&1 | while read -r line; …)" …)` — an
//! external-producer pipeline inside command substitution. huck used to run
//! completer subprocesses/pipelines with job control (setpgid +
//! give_terminal_to / tcsetpgrp) while mid-line-edit (raw mode), which wedged
//! the shell. bash runs completion functions WITHOUT job control.
//!
//! This spawns huck under a real PTY, sources the system bash-completion, then
//! triggers `ls -<TAB>`. A hard per-read timeout turns a hang into a failed
//! `expect` (the sentinel never arrives) rather than wedging the whole suite;
//! dropping the session kills the wedged child. Skips (passes) if no PTY can be
//! allocated OR if the system bash-completion script is absent.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use expectrl::session::OsSession;
use expectrl::Expect;

const BASH_COMPLETION: &str = "/usr/share/bash-completion/bash_completion";

#[test]
fn external_pipeline_completer_does_not_hang() {
    if !Path::new(BASH_COMPLETION).exists() {
        eprintln!("completion_jobcontrol_pty: skipping — no {BASH_COMPLETION}");
        return;
    }

    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    let mut session = match OsSession::spawn(cmd) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("completion_jobcontrol_pty: skipping — no PTY: {e}");
            return;
        }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));

    // 1. Source system bash-completion (registers `_longopt` for `ls` et al.).
    //    Expect a sentinel on the SAME line so the read stream stays in sync
    //    before we send the next line (back-to-back sends without an
    //    intervening read desync expectrl's incremental buffer).
    let _ = session.send("source /usr/share/bash-completion/bash_completion; echo SETUP_OK_$((6*7))");
    let _ = session.send("\r");
    assert!(
        session.expect("SETUP_OK_42").is_ok(),
        "setup never completed (shell dead before TAB, or bash-completion failed to source)"
    );

    // 2. Trigger completion: `ls -` then TAB (no CR). `_longopt` runs
    //    `ls --help | while read …` inside `$()`. Pre-fix this hangs (the
    //    completer pipeline setpgid's + hands off the terminal mid-line-edit).
    let _ = session.send("ls -\t");
    // 3. Clear the line (Ctrl-U) + fresh sentinel. If TAB hung, never processed.
    let _ = session.send("\x15");
    let _ = session.send("echo TAB_DONE_$((7*8))\r");
    let responsive = session.expect("TAB_DONE_56").is_ok();

    drop(session);
    assert!(
        responsive,
        "TAB completion hung: shell unresponsive after invoking an external-pipeline completer (M-116)"
    );
}
