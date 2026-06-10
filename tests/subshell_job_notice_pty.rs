//! v128: automatic `&` job notices (`[N] pid`, `[N]- Done … &`) must be
//! SUPPRESSED inside a subshell environment (matching bash) but STILL printed
//! for a top-level `&`. nvm's alias loops are `( … & … wait ) | sort`.
//! Skips (passes) if no PTY.

use std::process::Command;
use std::time::Duration;
use expectrl::session::OsSession;
use expectrl::Expect;

fn run_in_pty(cmd: &str) -> Option<String> {
    let mut session = match OsSession::spawn(Command::new(env!("CARGO_BIN_EXE_huck"))) {
        Ok(s) => s,
        Err(e) => { eprintln!("subshell_job_notice_pty: skipping — no PTY: {e}"); return None; }
    };
    session.set_expect_timeout(Some(Duration::from_secs(8)));
    let _ = session.send("echo READY_$((6*7))");
    let _ = session.send("\r");
    let _ = session.expect("READY_42");
    let _ = session.send(cmd);
    let _ = session.send("; echo MK_$((7*8))\r");
    let buf = match session.expect("MK_56") {
        Ok(found) => String::from_utf8_lossy(found.before()).into_owned(),
        Err(_) => String::new(),
    };
    drop(session);
    Some(buf)
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                while let Some(&n) = it.peek() { it.next(); if ('@'..='~').contains(&n) { break; } }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn job_notice_lines(out: &str) -> usize {
    strip_ansi(out).lines().filter(|l| {
        let t = l.trim_start();
        t.starts_with('[') && t[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
    }).count()
}

#[test]
fn subshell_background_job_emits_no_notice() {
    let Some(out) = run_in_pty("( sleep 0.05 & wait )") else { return };
    assert_eq!(job_notice_lines(&out), 0, "subshell `&` must not notify; got:\n{out}");
}
#[test]
fn subshell_pipeline_background_job_emits_no_notice() {
    let Some(out) = run_in_pty("( sleep 0.05 & wait ) | cat") else { return };
    assert_eq!(job_notice_lines(&out), 0, "subshell|pipe `&` must not notify; got:\n{out}");
}
#[test]
fn top_level_background_job_still_notifies() {
    let Some(out) = run_in_pty("sleep 0.05 & wait") else { return };
    assert!(job_notice_lines(&out) >= 1, "top-level `&` must still notify; got:\n{out}");
}
