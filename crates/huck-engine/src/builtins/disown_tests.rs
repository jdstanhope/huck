use super::*;
use crate::shell_state::Shell;

#[test]
fn is_builtin_recognizes_disown() {
    assert!(is_builtin("disown"));
}

#[test]
fn disown_no_args_with_no_current_job_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("disown", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn disown_no_args_removes_current_job() {
    let mut shell = Shell::new();
    shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("disown", &[], &mut buf, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.jobs.iter().count(), 0);
}

#[test]
fn disown_with_spec_removes_specified_job() {
    let mut shell = Shell::new();
    shell.jobs.add(100, vec![100], "a".to_string());
    shell.jobs.add(200, vec![200], "b".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let remaining: Vec<u32> = shell.jobs.iter().map(|j| j.id).collect();
    assert_eq!(remaining, vec![2]);
}

#[test]
fn disown_with_bad_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["%abc".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn disown_with_non_percent_arg_returns_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn disown_drops_pending_done_notification() {
    let mut shell = Shell::new();
    // Synthetic Done job with notified=false would trigger a "[1] Done"
    // line at the next prompt. Disown should remove the job and
    // suppress that notification.
    shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.jobs.iter().count(), 0);
}

use crate::jobs::JobState;

#[test]
fn disown_a_removes_all_jobs() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("a".to_string(), 0);
    shell.jobs.add_synthetic_done("b".to_string(), 0);
    shell.jobs.add_synthetic_done("c".to_string(), 0);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-a".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.jobs.iter().count(), 0);
}

#[test]
fn disown_r_filters_to_running_only() {
    let mut shell = Shell::new();
    // 2 Running + 1 Done — verifies bare `disown -r` removes BOTH
    // running jobs (bash semantics), not just the current.
    shell.jobs.add(1234, vec![1234], "sleep a".to_string()); // %1 Running
    shell.jobs.add(1235, vec![1235], "sleep b".to_string()); // %2 Running
    shell.jobs.add_synthetic_done("c".to_string(), 0); // %3 Done
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-r".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    // Both Running jobs gone; only %3 (Done) remains.
    let states: Vec<JobState> = shell.jobs.iter().map(|j| j.state.clone()).collect();
    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], JobState::Done(_)));
}

#[test]
fn disown_h_marks_for_nohup_keeps_in_table() {
    let mut shell = Shell::new();
    let id = shell.jobs.add(1234, vec![1234], "sleep".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-h".to_string(), "%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let job = shell
        .jobs
        .iter()
        .find(|j| j.id == id)
        .expect("job removed!");
    assert!(job.marked_for_nohup);
}

#[test]
fn disown_multiple_args_processes_each() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("a".to_string(), 0); // %1
    shell.jobs.add_synthetic_done("b".to_string(), 0); // %2
    shell.jobs.add_synthetic_done("c".to_string(), 0); // %3
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["%1".to_string(), "%2".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let ids: Vec<u32> = shell.jobs.iter().map(|j| j.id).collect();
    assert_eq!(ids, vec![3]);
}

#[test]
fn disown_ah_marks_all() {
    let mut shell = Shell::new();
    let id1 = shell.jobs.add(1234, vec![1234], "a".to_string());
    let id2 = shell.jobs.add(1235, vec![1235], "b".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-ah".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.jobs.iter().count(), 2);
    assert!(
        shell
            .jobs
            .iter()
            .find(|j| j.id == id1)
            .unwrap()
            .marked_for_nohup
    );
    assert!(
        shell
            .jobs
            .iter()
            .find(|j| j.id == id2)
            .unwrap()
            .marked_for_nohup
    );
}

#[test]
fn disown_ar_removes_all_running() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "a".to_string()); // %1 Running
    shell.jobs.add(1235, vec![1235], "b".to_string()); // %2 Running
    shell.jobs.add_synthetic_done("c".to_string(), 0); // %3 Done
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-ar".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let states: Vec<JobState> = shell.jobs.iter().map(|j| j.state.clone()).collect();
    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], JobState::Done(_)));
}

#[test]
fn disown_arh_marks_all_running() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "a".to_string()); // %1 Running
    shell.jobs.add(1235, vec![1235], "b".to_string()); // %2 Running
    shell.jobs.add_synthetic_done("c".to_string(), 0); // %3 Done
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-arh".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.jobs.iter().count(), 3);
    for job in shell.jobs.iter() {
        match job.state {
            JobState::Running => assert!(job.marked_for_nohup, "running job not marked"),
            _ => assert!(!job.marked_for_nohup, "non-running job got marked"),
        }
    }
}

#[test]
fn disown_invalid_flag_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-x".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn disown_a_ignores_positional_args() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "a".to_string()); // %1
    shell.jobs.add(1235, vec![1235], "b".to_string()); // %2
    shell.jobs.add(1236, vec![1236], "c".to_string()); // %3
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-a".to_string(), "%1".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.jobs.iter().count(), 0);
}

#[test]
fn disown_bare_pid_matches_job_leader() {
    let mut shell = Shell::new();
    shell.jobs.add(1234, vec![1234], "sleep".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["1234".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.jobs.iter().count(), 0);
}

#[test]
fn disown_bare_pid_matches_pipeline_stage() {
    let mut shell = Shell::new();
    shell
        .jobs
        .add(1234, vec![1234, 1235, 1236], "a | b | c".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["1235".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(shell.jobs.iter().count(), 0);
}

#[test]
fn disown_unknown_pid_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["99999".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn disown_h_with_bare_pid_marks_job() {
    let mut shell = Shell::new();
    let id = shell.jobs.add(1234, vec![1234], "sleep".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "disown",
        &["-h".to_string(), "1234".to_string()],
        &mut buf,
        &mut std::io::stderr(),
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    let job = shell
        .jobs
        .iter()
        .find(|j| j.id == id)
        .expect("job removed!");
    assert!(job.marked_for_nohup);
}
