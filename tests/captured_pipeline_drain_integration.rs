//! v133: a captured pipeline whose output exceeds the pipe buffer must not
//! deadlock (M-119). Each run is wrapped in a watchdog that kills the child
//! after a timeout, so a regression FAILS as a timeout rather than hanging the
//! test run.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// SIGKILL the entire descendant tree rooted at `root` (BFS via `pgrep -P`),
/// then `root` itself. A hung huck spawns its pipeline stages in their OWN
/// process group (pgid = first-stage pid, distinct from huck's pid/group), so
/// killing huck alone orphans those stages — they linger blocked on pipes
/// (reparented to PID 1). Killing the whole tree prevents the leak.
fn kill_process_tree(root: u32) {
    let mut pids = vec![root];
    let mut i = 0;
    while i < pids.len() {
        let pid = pids[i];
        i += 1;
        if let Ok(o) = Command::new("pgrep").arg("-P").arg(pid.to_string()).output() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Ok(child) = line.trim().parse::<u32>() {
                    pids.push(child);
                }
            }
        }
    }
    for pid in pids {
        // SAFETY: `kill(pid, SIGKILL)` is an always-safe syscall; an already-dead
        // or reparented pid just yields ESRCH, which we ignore.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL); }
    }
}

/// Runs `script` through huck with a `secs` watchdog. Returns
/// `Some((stdout, stderr, code))` on normal completion, `None` if it hung.
fn run_guarded(script: &str, secs: u64) -> Option<(String, String, i32)> {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    let pid = child.id();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let (tx, rx) = mpsc::channel::<()>();
    let wd = thread::spawn(move || -> bool {
        if rx.recv_timeout(Duration::from_secs(secs)).is_err() {
            kill_process_tree(pid);
            true
        } else {
            false
        }
    });
    let out = child.wait_with_output().unwrap();
    let _ = tx.send(());
    let killed = wd.join().unwrap();
    if killed {
        None
    } else {
        Some((
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        ))
    }
}

#[test]
fn large_captured_pipeline_does_not_hang() {
    let r = run_guarded("x=$(seq 1 500000 | cat); echo ${#x}\n", 10);
    let (o, _e, _c) = r.expect("HUNG: captured large pipeline deadlocked");
    assert_eq!(o.trim(), "3388894", "o: {o:?}");
}

#[test]
fn three_stage_captured_pipeline() {
    let r = run_guarded("x=$(seq 1 200000 | cat | cat); echo ${#x}\n", 10);
    let (o, _e, _c) = r.expect("HUNG: 3-stage captured pipeline deadlocked");
    // sum(len(n)+1) counts a newline after every number, but $() strips the
    // trailing newline, so subtract 1. Confirmed against bash: 1288894.
    let expected = (1..=200000).map(|n| n.to_string().len() + 1).sum::<usize>() - 1;
    assert_eq!(o.trim(), expected.to_string(), "o: {o:?}");
}

#[test]
fn small_captured_pipeline_still_works() {
    let r = run_guarded("x=$(seq 1 1000 | cat); echo ${#x}\n", 10);
    let (o, _e, _c) = r.expect("hung");
    assert_eq!(o.trim(), "3892", "o: {o:?}");
}

#[test]
fn large_producer_small_final_output() {
    let r = run_guarded("x=$(seq 1 500000 | wc -l); echo \"[$x]\"\n", 10);
    let (o, _e, _c) = r.expect("hung");
    // BSD `wc -l` (macOS) pads its output with leading spaces; GNU `wc -l`
    // (Linux) doesn't. The padding survives `$()` and lands inside the
    // brackets, so compare the inner numeric value with the spaces
    // trimmed rather than asserting exact bracket contents.
    let inner = o.trim().trim_start_matches('[').trim_end_matches(']').trim();
    assert_eq!(inner, "500000", "o: {o:?}");
}

#[test]
fn non_capture_pipeline_unaffected() {
    let r = run_guarded("seq 1 100 | wc -l\n", 10);
    let (o, _e, _c) = r.expect("hung");
    assert_eq!(o.trim(), "100", "o: {o:?}");
}

#[test]
fn pipestatus_after_captured_pipeline() {
    let r = run_guarded("x=$(false | true); echo \"[${PIPESTATUS[*]}]\"\n", 10);
    let (o, _e, _c) = r.expect("hung");
    assert_eq!(o.trim(), "[0]", "o: {o:?} — if bash differs, set expected to bash's output");
}
