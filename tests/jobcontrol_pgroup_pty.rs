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

use expectrl::Expect;
use expectrl::session::OsSession;

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

/// L-51: a non-interactive huck running a multi-stage pipeline must keep its
/// stages in huck's OWN process group, not a sibling group.
#[test]
fn noninteractive_pipeline_stages_share_shell_pgroup() {
    let huck = env!("CARGO_BIN_EXE_huck");

    // `sleep 30 | wc -c`: a two-stage pipeline whose first stage stays alive
    // (sleeping) so both stages are observable while we inspect process groups,
    // and which self-terminates — a missed stage can never become a permanent
    // orphan. (The old `cat <fifo>` form blocked forever, so any cleanup miss
    // leaked a cat/wc pair indefinitely.) Non-interactive: stdio not a tty.
    let mut child = Command::new(huck)
        .arg("-c")
        .arg("sleep 30 | wc -c")
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

    // Cleanup before asserting (so a failed assert never leaks processes). Kill
    // the whole descendant tree, not just direct children — a pipeline's stages
    // can be grandchildren (under a pipeline subshell), which `pgrep -P` misses.
    // Capture descendants BEFORE killing huck (they reparent to init after).
    let descendants = descendant_pids(huck_pid);
    let _ = child.kill();
    for &p in &descendants {
        let _ = Command::new("kill")
            .args(["-9", &p.to_string()])
            .stderr(Stdio::null())
            .status();
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
    // `sleep 30 | wc -c &`: a BACKGROUND two-stage pipeline; the trailing `sleep`
    // keeps huck alive while we inspect. Self-terminating stages (no fifo) so a
    // cleanup miss can't leak indefinitely — see the L-51 test above.
    let mut child = Command::new(huck)
        .arg("-c")
        .arg("sleep 30 | wc -c & sleep 30")
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

    // Kill the whole descendant tree — a background pipeline's stages can be
    // grandchildren (under a pipeline subshell), which `pgrep -P` misses.
    // Capture descendants BEFORE killing huck (they reparent to init after).
    let descendants = descendant_pids(huck_pid);
    let _ = child.kill();
    for &p in &descendants {
        let _ = Command::new("kill")
            .args(["-9", &p.to_string()])
            .stderr(Stdio::null())
            .status();
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
        let _ = Command::new("kill")
            .args(["-9", &p.to_string()])
            .stderr(Stdio::null())
            .status();
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
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse()
            .unwrap_or(-1),
        Err(_) => -2,
    }
}

/// Direct child pids of `parent` via `pgrep -P`.
fn child_pids(parent: u32) -> Vec<u32> {
    let out = Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// All descendant pids of `parent` (breadth-first via `pgrep -P`), excluding
/// `parent` itself. Cleanup needs this — a backgrounded pipeline's stages can be
/// grandchildren under a pipeline subshell, which `child_pids` (direct only)
/// would miss, orphaning them.
fn descendant_pids(parent: u32) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut frontier = vec![parent];
    while let Some(p) = frontier.pop() {
        for c in child_pids(p) {
            if !out.contains(&c) {
                out.push(c);
                frontier.push(c);
            }
        }
    }
    out
}
