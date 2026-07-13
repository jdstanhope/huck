//! Integration coverage for `disown` on background jobs at shell exit.
//!
//! Since #128 (v289), a **non-interactive** shell (these tests drive huck with a
//! piped stdin, so `is_interactive` is false) never SIGHUPs its jobs at exit —
//! matching bash, which only hangs up jobs when an interactive shell exits with
//! the `huponexit` shopt set. So every background job here survives exit
//! regardless of `disown`; these tests confirm that `disown`/`disown -h`/`disown
//! -ah` do not error and that the job is left alive.
//!
//! The `disown -h` *exemption* itself (a marked job is skipped by the exit-hangup
//! loop) is exercised directly by the `should_hangup_skips_marked_and_done_jobs`
//! unit test in `huck-engine/src/shell_state/tests.rs`, which does not need the
//! interactive+huponexit hangup path.

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// True if a process with this pid is alive AND signalable from
/// the test process (`libc::kill(pid, 0)` returns 0).
fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Best-effort cleanup: send SIGTERM to a PID that may or may not
/// still be alive.
fn cleanup_kill(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// Parses the first positive integer from a string. Used to pull a
/// PID out of `jobs -p` output.
fn first_pid(s: &str) -> Option<i32> {
    for word in s.split_whitespace() {
        if let Ok(n) = word.parse::<i32>()
            && n > 0
        {
            return Some(n);
        }
    }
    None
}

#[test]
fn disown_h_lets_bg_job_survive() {
    // sleep 30 >/dev/null 2>&1 & ; echo $! ; disown -h %1 ; exit
    // Note: huck's `jobs` does not accept `-p`, so we capture the bg PID
    // via $! (last-bg-pid) instead.
    let script = "sleep 30 >/dev/null 2>&1 &\necho $!\ndisown -h %1\nexit\n";
    let (out, _) = run_capture(script);
    let pid = first_pid(&out).unwrap_or_else(|| panic!("no pid found in: {:?}", out));
    thread::sleep(Duration::from_millis(200));
    let alive = pid_alive(pid);
    cleanup_kill(pid);
    assert!(alive, "bg job (pid {pid}) was killed despite disown -h");
}

#[test]
fn bg_job_survives_noninteractive_exit_without_disown() {
    // #128: a non-interactive shell must NOT SIGHUP its jobs at exit, even without
    // `disown` — matching bash (which only hangs up jobs for an interactive shell
    // exiting with `huponexit` set). The bg job must still be alive after exit.
    let script = "sleep 30 >/dev/null 2>&1 &\necho $!\nexit\n";
    let (out, _) = run_capture(script);
    let pid = first_pid(&out).unwrap_or_else(|| panic!("no pid found in: {:?}", out));
    thread::sleep(Duration::from_millis(200));
    let alive = pid_alive(pid);
    cleanup_kill(pid);
    assert!(
        alive,
        "bg job (pid {pid}) was SIGHUP'd on non-interactive exit; bash leaves it running (#128)"
    );
}

#[test]
fn disown_a_h_marks_all_alive() {
    let script = "sleep 30 >/dev/null 2>&1 &\necho $!\nsleep 30 >/dev/null 2>&1 &\necho $!\ndisown -ah\nexit\n";
    let (out, _) = run_capture(script);
    let pids: Vec<i32> = out
        .split_whitespace()
        .filter_map(|w| w.parse::<i32>().ok())
        .filter(|n| *n > 0)
        .collect();
    assert!(
        pids.len() >= 2,
        "expected >= 2 pids in stdout, got {:?}",
        pids
    );
    thread::sleep(Duration::from_millis(200));
    let all_results: Vec<(i32, bool)> = pids.iter().map(|p| (*p, pid_alive(*p))).collect();
    // Cleanup regardless of assertion outcome.
    for &(pid, _) in &all_results {
        cleanup_kill(pid);
    }
    for (pid, alive) in &all_results {
        assert!(alive, "bg job (pid {pid}) was killed despite disown -ah");
    }
}
