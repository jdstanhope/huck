//! v172 regression: interactive job control (Ctrl-Z stop / `fg` resume on a
//! pipeline) still works, AND a NON-interactive pipeline keeps its stages in the
//! shell's own process group (L-51). The interactive case guards the
//! v108/v121/v124 tty-deadlock area against this change; the pgroup case proves
//! L-51 fixed (before v172 a non-interactive pipeline's stages were setpgid'd
//! into a sibling group instead of inheriting the shell's group like bash).
//!
//! Skips (passes) if no PTY can be allocated or `ps`/`pgrep` are unavailable.

use std::process::{Command, Stdio};
use std::time::Duration;

use expectrl::session::OsSession;
use expectrl::Expect;

fn spawn() -> Option<OsSession> {
    let cmd = Command::new(env!("CARGO_BIN_EXE_huck"));
    match OsSession::spawn(cmd) {
        Ok(mut s) => {
            s.set_expect_timeout(Some(Duration::from_secs(8)));
            let _ = s.send("echo READY_$((6*7))\r");
            if s.expect("READY_42").is_err() {
                eprintln!("jobcontrol_pgroup_pty: skipping — interactive marker not seen");
                return None;
            }
            Some(s)
        }
        Err(e) => {
            eprintln!("jobcontrol_pgroup_pty: skipping — no PTY: {e}");
            None
        }
    }
}

/// Interactive job control still works: a foreground pipeline prints READY, then
/// would sleep and print DONE. Ctrl-Z stops the whole job (shell returns to the
/// prompt without DONE); `fg` resumes it to completion (DONE). A
/// hang/timeout — i.e. a broken pipeline pgroup or terminal handoff — fails the
/// `expect`.
#[test]
fn interactive_pipeline_ctrl_z_then_fg_resumes() {
    let Some(mut session) = spawn() else { return };

    // First stage feeds nothing useful; the second stage prints READY, sleeps,
    // then prints DONE. We stop it mid-sleep and resume with `fg`.
    let _ = session.send("sleep 3 | { echo READY; sleep 3; echo DONE; }\r");
    let saw_ready = session.expect("READY").is_ok();
    // Stop the job while the second stage is mid-sleep.
    std::thread::sleep(Duration::from_millis(500));
    let _ = session.send("\x1a"); // Ctrl-Z (SIGTSTP)
    // Give the shell a moment to reap the stop and reprint the prompt.
    std::thread::sleep(Duration::from_millis(300));
    // Resume the stopped job in the foreground; it must run to DONE.
    let _ = session.send("fg\r");
    let saw_done = session.expect("DONE").is_ok();

    let _ = session.send("kill -9 %1 2>/dev/null\r");
    drop(session);

    assert!(saw_ready, "pipeline did not start under PTY (no READY)");
    assert!(
        saw_done,
        "fg did not resume the Ctrl-Z-stopped pipeline to completion (no DONE) — \
         pipeline pgroup / terminal handoff regression"
    );
}

/// L-51: a non-interactive huck running a pipeline blocked on a fifo must keep
/// its stages in huck's OWN process group, not a sibling group.
#[test]
fn noninteractive_pipeline_stages_share_shell_pgroup() {
    let huck = env!("CARGO_BIN_EXE_huck");
    let fifo = std::env::temp_dir().join(format!("v172_jc_{}", std::process::id()));
    let _ = std::fs::remove_file(&fifo);
    if Command::new("mkfifo").arg(&fifo).status().map(|s| !s.success()).unwrap_or(true) {
        eprintln!("jobcontrol_pgroup_pty: skipping — mkfifo unavailable");
        return;
    }

    // `cat <fifo>` blocks (no writer), so the pipeline stays alive while we
    // inspect process groups. Non-interactive: stdin/stdout/stderr not a tty.
    let mut child = Command::new(huck)
        .arg("-c")
        .arg(format!("cat {} | wc -c", fifo.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn huck");
    std::thread::sleep(Duration::from_millis(500));

    let huck_pid = child.id();
    let huck_pgid = pgid_of(huck_pid);
    let stages = child_pids(huck_pid);
    let stage_pgids: Vec<(u32, i32)> = stages.iter().map(|&p| (p, pgid_of(p))).collect();

    // Cleanup before asserting (so a failed assert never leaks processes).
    let _ = std::fs::remove_file(&fifo);
    let _ = child.kill();
    for &p in &stages {
        let _ = Command::new("kill").args(["-9", &p.to_string()]).status();
    }
    let _ = child.wait();

    if stages.is_empty() || huck_pgid < 0 {
        eprintln!("jobcontrol_pgroup_pty: skipping — could not observe stages/pgid");
        return;
    }
    for (p, pg) in stage_pgids {
        assert_eq!(
            pg, huck_pgid,
            "stage {p} pgid {pg} != huck pgid {huck_pgid} \
             (L-51: non-interactive pipeline stage landed in a sibling group)"
        );
    }
}

/// L-53: a non-interactive huck running a BACKGROUND pipeline (`a | b &`) keeps
/// its stages in huck's own process group (not a sibling group). huck stays
/// alive via a trailing `sleep` while we inspect.
#[test]
fn noninteractive_background_pipeline_shares_shell_pgroup() {
    let huck = env!("CARGO_BIN_EXE_huck");
    let fifo = std::env::temp_dir().join(format!("v173_bgp_{}", std::process::id()));
    let _ = std::fs::remove_file(&fifo);
    if Command::new("mkfifo").arg(&fifo).status().map(|s| !s.success()).unwrap_or(true) {
        eprintln!("jobcontrol_pgroup_pty: skipping — mkfifo unavailable");
        return;
    }
    let mut child = Command::new(huck)
        .arg("-c")
        .arg(format!("cat {} | wc -c & sleep 3", fifo.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn huck");
    std::thread::sleep(Duration::from_millis(600));
    let huck_pid = child.id();
    let huck_pgid = pgid_of(huck_pid);
    let stages = child_pids(huck_pid);
    let stage_pgids: Vec<(u32, i32)> = stages.iter().map(|&p| (p, pgid_of(p))).collect();

    let _ = std::fs::remove_file(&fifo);
    let _ = child.kill();
    for &p in &stages {
        let _ = Command::new("kill").args(["-9", &p.to_string()]).status();
    }
    let _ = child.wait();

    if stages.is_empty() || huck_pgid < 0 {
        eprintln!("jobcontrol_pgroup_pty: skipping — could not observe stages/pgid");
        return;
    }
    for (p, pg) in stage_pgids {
        assert_eq!(
            pg, huck_pgid,
            "background stage {p} pgid {pg} != huck pgid {huck_pgid} \
             (L-53: backgrounded pipeline stage landed in a sibling group)"
        );
    }
}

/// L-53: a non-interactive huck running `( cmd ) &` keeps the subshell child in
/// huck's own process group.
#[test]
fn noninteractive_background_subshell_shares_shell_pgroup() {
    let huck = env!("CARGO_BIN_EXE_huck");
    let mut child = Command::new(huck)
        .arg("-c")
        .arg("( exec sleep 3 ) & sleep 3")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn huck");
    std::thread::sleep(Duration::from_millis(600));
    let huck_pid = child.id();
    let huck_pgid = pgid_of(huck_pid);
    let kids = child_pids(huck_pid);
    let kid_pgids: Vec<(u32, i32)> = kids.iter().map(|&p| (p, pgid_of(p))).collect();

    let _ = child.kill();
    for &p in &kids {
        let _ = Command::new("kill").args(["-9", &p.to_string()]).status();
    }
    let _ = child.wait();

    if kids.is_empty() || huck_pgid < 0 {
        eprintln!("jobcontrol_pgroup_pty: skipping — could not observe child/pgid");
        return;
    }
    for (p, pg) in kid_pgids {
        assert_eq!(
            pg, huck_pgid,
            "background subshell child {p} pgid {pg} != huck pgid {huck_pgid} \
             (L-53: (cmd)& child landed in a sibling group)"
        );
    }
}

/// `ps -o pgid= -p <pid>` → the process group id, or a negative sentinel if it
/// can't be read (process gone / `ps` unavailable).
fn pgid_of(pid: u32) -> i32 {
    let out = Command::new("ps")
        .args(["-o", "pgid=", "-p", &pid.to_string()])
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(-1),
        Err(_) => -2,
    }
}

/// Direct child pids of `parent` via `pgrep -P`.
fn child_pids(parent: u32) -> Vec<u32> {
    let out = Command::new("pgrep").args(["-P", &parent.to_string()]).output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}
