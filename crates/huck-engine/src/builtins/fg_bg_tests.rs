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
