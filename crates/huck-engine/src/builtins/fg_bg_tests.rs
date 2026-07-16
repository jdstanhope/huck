use super::*;
use crate::shell_state::Shell;

#[test]
fn fg_with_no_jobs_errors() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("fg", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_with_no_jobs_errors() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("bg", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn fg_with_percent_spec_arg_and_no_job_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "fg",
        &["%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_with_percent_spec_arg_and_no_job_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "bg",
        &["%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_on_running_job_returns_no_current_job() {
    let mut shell = Shell::new();
    shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("bg", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn fg_on_terminal_job_reports_no_such_job_and_drops_it() {
    // #162: a job the entry-reap already completed (Done) must be treated as
    // gone — bash reports "no such job" + removes it — rather than clobbered
    // back to Running (which leaked a phantom entry and returned 1 via a
    // waitpid(-pgid) ECHILD race).
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("sleep 0.05".to_string(), 0);
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let outcome = run_builtin("fg", &["%1".to_string()], &mut out, &mut err, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(
        String::from_utf8_lossy(&err).contains("fg: %1: no such job"),
        "stderr: {}",
        String::from_utf8_lossy(&err)
    );
    assert!(
        shell.jobs.iter().next().is_none(),
        "the completed job must be removed, not left as a phantom Running entry"
    );
}

#[test]
fn fg_on_signaled_job_reports_no_such_job_and_drops_it() {
    // Same as above for a Signaled terminal state.
    let mut shell = Shell::new();
    shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
    shell.jobs.jobs_mut()[0].state = crate::jobs::JobState::Signaled(libc::SIGKILL);
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let outcome = run_builtin("fg", &["%1".to_string()], &mut out, &mut err, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(
        String::from_utf8_lossy(&err).contains("fg: %1: no such job"),
        "stderr: {}",
        String::from_utf8_lossy(&err)
    );
    assert!(shell.jobs.iter().next().is_none());
}

#[test]
fn bg_on_terminal_job_reports_no_such_job_and_drops_it() {
    // #162: bg on a job the entry-reap completed must report "no such job" +
    // remove it, not misreport "already running" (the pre-fix behavior).
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("sleep 0.05".to_string(), 0);
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let outcome = run_builtin("bg", &["%1".to_string()], &mut out, &mut err, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
    assert!(
        String::from_utf8_lossy(&err).contains("bg: %1: no such job"),
        "stderr: {}",
        String::from_utf8_lossy(&err)
    );
    assert!(shell.jobs.iter().next().is_none());
}

#[test]
fn is_builtin_recognizes_fg_and_bg() {
    assert!(is_builtin("fg"));
    assert!(is_builtin("bg"));
}

#[test]
fn fg_with_bad_job_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "fg",
        &["%abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn fg_with_no_such_job_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "fg",
        &["%99".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn fg_with_non_percent_arg_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "fg",
        &["1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn fg_with_multiple_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "fg",
        &["%1".to_string(), "%2".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn bg_with_bad_job_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "bg",
        &["%abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_with_no_such_job_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "bg",
        &["%99".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_with_running_spec_errors_already_running() {
    let mut shell = Shell::new();
    shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "bg",
        &["%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_with_multiple_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "bg",
        &["%1".to_string(), "%2".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn wait_with_bad_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["%abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn wait_with_no_such_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["%99".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn wait_multiarg_unparseable_returns_status_1() {
    // A malformed spec is a spec error, not a usage error: bash returns 1 (#161).
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["1234".to_string(), "abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn wait_with_unparseable_pid_arg_returns_status_1() {
    // bash: `wait: `abc': not a pid or valid job spec` → rc 1 (#161).
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn wait_with_done_spec_returns_decoded_status_immediately() {
    let mut shell = Shell::new();
    // Synthetic Done job — wait should see it's already terminal and
    // return decode(0) → 0 without blocking.
    shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn wait_with_done_spec_returns_nonzero_for_exit_n() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("false".to_string(), 1);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn wait_multiarg_two_done_returns_last_status() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("true".to_string(), 0);
    shell.jobs.add_synthetic_done("exit 5".to_string(), 5);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["%1".to_string(), "%2".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(5)));
}

#[test]
fn wait_multiarg_unparseable_rejects_before_waiting() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("true".to_string(), 0);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["%1".to_string(), "abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    // Rejected on the bad spec (rc 1, #161) before waiting on %1.
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn wait_n_with_no_jobs_returns_127() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["-n".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(127)));
}

#[test]
fn wait_n_with_only_done_jobs_returns_127() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("true".to_string(), 0);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["-n".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(127)));
}

#[test]
fn wait_n_with_explicit_already_done_returns_its_status() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("exit 7".to_string(), 7);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["-n".to_string(), "%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(7)));
}

#[test]
fn wait_n_p_var_captures_pgid_via_explicit_target() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("true".to_string(), 0);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &[
            "-n".to_string(),
            "-p".to_string(),
            "PID".to_string(),
            "%1".to_string(),
        ],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.lookup_var("PID").as_deref(), Some("0"));
}

#[test]
fn wait_p_without_n_is_usage_error() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["-p".to_string(), "PID".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn wait_n_p_without_var_name_is_usage_error() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["-n".to_string(), "-p".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn reap_and_notify_prunes_done_job_non_interactively() {
    // #175: a completed (Done) job is silently removed from the table by the
    // between-command maintenance pass (`reap_and_notify`), matching bash's
    // non-interactive pruning. No print because the shell is non-interactive.
    let mut shell = Shell::new();
    shell.is_interactive = false;
    shell.jobs.add_synthetic_done("sleep 0".to_string(), 0);
    assert_eq!(shell.jobs.iter().count(), 1);
    crate::jobs::reap_and_notify(&mut shell);
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
    shell.jobs.jobs_mut()[0].state = crate::jobs::JobState::Stopped(libc::SIGTSTP);
    crate::jobs::reap_and_notify(&mut shell);
    assert!(
        shell.jobs.iter().any(|j| j.id == id),
        "a Stopped job must be kept, not pruned"
    );
}

#[test]
fn saved_status_ring_records_looks_up_and_evicts_oldest() {
    // #175 follow-up: the saved-status ring retains a completed job's exit code
    // by pid so `wait $pid` resolves after the visible job was pruned. It is
    // bounded (drop-oldest) so it cannot re-introduce an unbounded leak.
    let mut table = crate::jobs::JobTable::new();
    table.record_terminal_status(100, 0);
    table.record_terminal_status(101, 7);
    assert_eq!(table.saved_status(100), Some(0));
    assert_eq!(table.saved_status(101), Some(7));
    assert_eq!(table.saved_status(999), None);
    // Re-recording a known pid refreshes its code in place (no duplicate).
    table.record_terminal_status(100, 3);
    assert_eq!(table.saved_status(100), Some(3));
    // Overflow past the cap (4096) evicts the oldest entries first. pid 100/101
    // were the two oldest, so after inserting 4096 fresh pids they are gone but
    // recent ones survive.
    for pid in 1000..(1000 + 4096) {
        table.record_terminal_status(pid, 1);
    }
    assert_eq!(
        table.saved_status(100),
        None,
        "oldest entry must be evicted"
    );
    assert_eq!(
        table.saved_status(101),
        None,
        "oldest entry must be evicted"
    );
    assert_eq!(table.saved_status(1000 + 4095), Some(1), "newest kept");
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
    shell.jobs.jobs_mut()[0].state = crate::jobs::JobState::Done(4);
    crate::jobs::reap_and_notify(&mut shell);
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

#[test]
fn wait_for_job_removes_its_id_from_table() {
    // #175: `wait %n` on a terminal job returns its status and removes the
    // entry, so a following `jobs` does not show it.
    let mut shell = Shell::new();
    shell.is_interactive = false;
    let id = shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &[format!("%{id}")],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert!(
        !shell.jobs.iter().any(|j| j.id == id),
        "wait_for_job must remove the waited job's id"
    );
}

#[test]
fn wait_invalid_flag_is_usage_error() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["-x".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

// ---- #167: `set -m` activates job control non-interactively -----------------

#[test]
fn job_control_active_true_under_monitor_noninteractive() {
    let mut shell = Shell::new();
    shell.is_interactive = false;
    shell.shell_options.monitor = true;
    assert!(
        shell.job_control_active(),
        "set -m must activate job control even when non-interactive"
    );
}

#[test]
fn job_control_active_false_when_neither_interactive_nor_monitor() {
    let mut shell = Shell::new();
    shell.is_interactive = false;
    shell.shell_options.monitor = false;
    assert!(
        !shell.job_control_active(),
        "job control must be inert with neither -i nor -m"
    );
}

#[test]
fn job_control_active_false_under_monitor_inside_subshell() {
    let mut shell = Shell::new();
    shell.is_interactive = false;
    shell.shell_options.monitor = true;
    shell.in_subshell = true;
    assert!(
        !shell.job_control_active(),
        "set -m job control must stay inert inside a subshell environment"
    );
}
