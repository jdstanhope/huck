//! #175's job-table pruning tests, in their OWN test binary so they never share
//! a process with a forking test.
//!
//! `jobs::reap_and_notify` calls `reap_completed`, which drains EVERY reapable
//! child of the process by looping on `waitpid(-1, WNOHANG | WUNTRACED |
//! WCONTINUED)`. That is correct for a shell — it owns its process — but it is
//! process-GLOBAL, and cargo runs a lib test binary's tests MULTITHREADED. In
//! the shared binary these tests therefore stole children forked by unrelated
//! tests running concurrently, making
//! `executor::tests::fork_and_run_in_subshell_echo_stage_writes_to_pipe` fail on
//! CI with `waitpid returned unexpected pid: left: -1` — ECHILD, because its
//! child had already been reaped here.
//!
//! Reproduced 1/40 runs with `--test-threads 4` on those tests together, and
//! 0/40 with the forking test alone; it never fires on a 1-core box, where
//! `--test-threads` defaults to 1. That is why it reached `main` green.
//!
//! These tests fork nothing, so in their own process the wildcard reap finds no
//! children (ECHILD) and returns at once — the pruning behavior under test is
//! unaffected. Same isolation rationale as `tee_inherit.rs` (#90).

use huck_engine::jobs;
use huck_engine::shell_state::Shell;

#[test]
fn reap_and_notify_prunes_done_job_non_interactively() {
    // #175: a completed (Done) job is silently removed from the table by the
    // between-command maintenance pass (`reap_and_notify`), matching bash's
    // non-interactive pruning. No print because the shell is non-interactive.
    let mut shell = Shell::new();
    shell.is_interactive = false;
    shell.jobs.add_synthetic_done("sleep 0".to_string(), 0);
    assert_eq!(shell.jobs.iter().count(), 1);
    jobs::reap_and_notify(&mut shell);
    assert!(
        shell.jobs.iter().next().is_none(),
        "the Done job must be pruned by reap_and_notify"
    );
}

#[test]
fn reap_and_notify_keeps_stopped_job() {
    // #175: Running/Stopped jobs are retained across the maintenance pass.
    let mut shell = Shell::new();
    shell.is_interactive = false;
    let id = shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
    shell.jobs.jobs_mut()[0].state = jobs::JobState::Stopped(libc::SIGTSTP);
    jobs::reap_and_notify(&mut shell);
    assert!(
        shell.jobs.iter().any(|j| j.id == id),
        "a Stopped job must be kept, not pruned"
    );
}

#[test]
fn remove_notified_records_terminal_status_for_later_wait() {
    // A Done job pruned by the between-command maintenance pass leaves its
    // exit status waitable by pid.
    let mut shell = Shell::new();
    shell.is_interactive = false;
    // Build a Running job with a real leader pid, then mark it Done so the prune
    // path records (pid -> code). add_synthetic_done has no pids, so use add.
    shell.jobs.add(555, vec![555], "sleep 0".to_string());
    shell.jobs.jobs_mut()[0].state = jobs::JobState::Done(4);
    jobs::reap_and_notify(&mut shell);
    assert!(
        shell.jobs.iter().next().is_none(),
        "the Done job must be pruned"
    );
    assert_eq!(
        shell.jobs.saved_status(555),
        Some(4),
        "pruned job's status must be retained for wait $pid"
    );
}
