//! Single-threaded isolation for tests that fork in-process (issue #184).
//!
//! huck runs subshells, background jobs, coprocesses, and in-process pipeline
//! stages by forking WITHOUT exec — the child continues in-process through
//! run_command (malloc, stdio), which is memory-safe only in a single-threaded
//! process. Under a parallel harness a concurrent thread can hold the
//! malloc/stdout lock at the fork instant and the child deadlocks; the
//! exec_guard turns that into a panic. So these CANNOT be parallel `#[test]`s —
//! each would fork while the others execute and trip the guard. They live here
//! as ONE `#[test]` running sequentially, the sole test in this binary, so no
//! sibling execution overlaps a fork. (Precedent: #90 / tee_inherit.rs;
//! streaming_fd_serial.rs.)
//!
//! Moved from lib #[cfg(test)] modules. Internal-state assertions (Shell.jobs,
//! Shell.get, the raw fork_and_run_in_subshell contract) are rewritten as
//! public-API behavioral proxies — same code paths, observed through Engine.
//!
//! The brief's task table names 7 tests; the Step-4 repro gate (running the
//! frozen lib binary 8x at --test-threads 4) surfaced two more in-process
//! forkers not caught by the original empirical scan: a background `&&`
//! chain (`execute_bg_chain_returns_immediately_status_0`) and a
//! capture-context builtin pipeline
//! (`execute_capturing_builtin_pipeline_captures_terminal_stage`, whose
//! non-terminal stage forks even though neither stage looks like a subshell).
//! Both moved here too, per the brief's "if an 8th forker surfaces, move it
//! too and repeat" instruction.

use huck_engine::Engine;

#[test]
fn forking_execution_scenarios() {
    subshell_stdout_and_stderr_captured_separately();
    nested_subshell_forks_single_threaded_without_tripping_the_guard();
    pipeline_last_stage_dispatches_on_stdout_line();
    subshell_pipeline_stage_writes_through_the_pipe();
    background_assignment_does_not_leak_to_parent();
    background_pure_builtin_sets_bang_pid();
    background_chain_registers_one_job();
    valid_coproc_name_starts_and_publishes();
    background_chain_returns_immediately_status_zero();
    builtin_pipeline_capture_returns_terminal_stage_output();
}

/// executor::tests::subshell_stderr_is_captured — stdout and stderr to separate sinks.
fn subshell_stdout_and_stderr_captured_separately() {
    let mut e = Engine::new();
    let out = e.capture("( echo out; echo err >&2 )");
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");
}

/// Guard same-thread re-entrancy: a nested subshell forks twice on one thread
/// (GLOBAL == LOCAL at each fork). Must not panic.
fn nested_subshell_forks_single_threaded_without_tripping_the_guard() {
    let mut e = Engine::new();
    let out = e.capture("( ( echo deep ) )");
    assert_eq!(out.stdout, "deep\n");
}

/// engine::tests::on_stdout_line_pipeline_last_stage — the on_stdout_line
/// callback fires for a pipeline's last stage. Moved verbatim (already public).
fn pipeline_last_stage_dispatches_on_stdout_line() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.prepare("echo hi | tr a-z A-Z")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert_eq!(lines, vec!["HI"]);
}

/// executor::tests::fork_and_run_in_subshell_echo_stage_writes_to_pipe —
/// behavioral proxy: an in-process subshell stage writes through a real pipe to
/// the next stage, driving fork_and_run_in_subshell with stdout → pipe.
fn subshell_pipeline_stage_writes_through_the_pipe() {
    let mut e = Engine::new();
    let out = e.capture("( echo hi-from-subshell ) | cat");
    assert_eq!(out.stdout, "hi-from-subshell\n");
}

/// executor::tests::background_pure_builtin_does_not_mutate_parent_env — a `&`
/// assignment runs in a forked subshell and must not leak to the parent.
fn background_assignment_does_not_leak_to_parent() {
    let mut e = Engine::new();
    let out = e.capture("HUCK_TEST_BG_ASSIGN=v & wait; echo [${HUCK_TEST_BG_ASSIGN-unset}]");
    assert_eq!(out.stdout, "[unset]\n");
}

/// executor::tests::background_pure_builtin_forks_and_registers_job — a
/// pure-builtin `&` forks and sets $! to a real positive pid.
fn background_pure_builtin_sets_bang_pid() {
    let mut e = Engine::new();
    let out = e.capture("echo hi >/dev/null & [ \"$!\" -gt 0 ] && echo haspid; wait");
    assert_eq!(out.stdout, "haspid\n");
}

/// executor::tests::execute_bg_chain_registers_job — `cmd && cmd &` registers
/// exactly one job. Cleans up the still-running sleep.
fn background_chain_registers_one_job() {
    let mut e = Engine::new();
    let out = e.capture("sleep 30 && true & jobs; kill %1 2>/dev/null; wait 2>/dev/null");
    let job_lines = out.stdout.lines().filter(|l| l.starts_with('[')).count();
    assert_eq!(
        job_lines, 1,
        "expected exactly one job; stdout: {:?}",
        out.stdout
    );
}

/// executor::coproc_name_tests::valid_coproc_name_still_starts — a valid coproc
/// name starts the coprocess (status 0) and publishes NAME_PID. Maps
/// `last_status()==0` + `coprocs[0].name=="MYCO"` from the original lib test
/// to the public `$?`/`$MYCO_PID`. The coproc body is `read _` (not `:`) so
/// it blocks and stays alive — a `:` body exits instantly and huck auto-reaps
/// a finished coproc at the next command boundary (v306 / #185), which raced
/// against the `$MYCO_PID` read; `$?`/`$MYCO_PID` are captured into variables
/// synchronously right after `coproc` to inspect state before any reap.
fn valid_coproc_name_starts_and_publishes() {
    let mut e = Engine::new();
    let out = e.capture(
        "coproc MYCO { read _; }; rc=$?; pid=$MYCO_PID; echo \"rc=$rc pid=$pid\"; kill \"$pid\" 2>/dev/null; wait 2>/dev/null",
    );
    let line = out
        .stdout
        .lines()
        .find(|l| l.starts_with("rc="))
        .unwrap_or("rc= pid=");
    let rc: i64 = line
        .strip_prefix("rc=")
        .and_then(|rest| rest.split(' ').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(-1);
    assert_eq!(
        rc, 0,
        "coproc should start, status 0; stdout: {:?}",
        out.stdout
    );
    let pid: i64 = line
        .split("pid=")
        .nth(1)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    assert!(
        pid > 0,
        "MYCO_PID should be a positive pid; stdout: {:?}",
        out.stdout
    );
}

/// executor::tests::execute_bg_chain_returns_immediately_status_0 — `cmd &&
/// cmd &` returns Continue(0) (the launch succeeded) without waiting for the
/// backgrounded chain to finish. Surfaced as an 8th in-process forker by the
/// Step-4 repro gate (issue #184): a background `&&` chain forks too, not
/// only a single background command.
fn background_chain_returns_immediately_status_zero() {
    let mut e = Engine::new();
    let out = e.capture("true && true & echo $?; wait 2>/dev/null");
    assert_eq!(out.stdout, "0\n");
}

/// executor::tests::execute_capturing_builtin_pipeline_captures_terminal_stage
/// — a capture-context builtin pipeline (`echo first | echo second`) forks
/// the non-terminal stage in-process; only the terminal stage's output lands
/// in the capture buffer. Surfaced as a 9th in-process forker by the Step-4
/// repro gate (issue #184).
fn builtin_pipeline_capture_returns_terminal_stage_output() {
    let mut e = Engine::new();
    let out = e.capture("echo \"$(echo first | echo second)\"");
    assert_eq!(out.stdout, "second\n");
}
