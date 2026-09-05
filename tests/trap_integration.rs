use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with `script` on stdin, captures stdout/stderr, returns
/// (stdout, stderr, exit_status).
fn run(script: &str) -> (String, String, std::process::ExitStatus) {
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
        out.status,
    )
}

/// Spawns huck with `script`, returns the child handle (still running)
/// + the pid. Caller is responsible for finishing the process.
fn spawn(script: &str) -> (std::process::Child, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    let pid = child.id() as i32;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    // No — we want huck to KEEP running until we send the signal.
    // Don't drop; the caller will manage.
    (child, pid)
}

/// Sends `signum` to `pid` via libc::kill.
fn send_signal(pid: i32, signum: i32) {
    unsafe {
        libc::kill(pid, signum);
    }
}

#[test]
fn exit_trap_fires_on_normal_exit() {
    let (out, _err, status) = run("trap 'echo bye' EXIT\nexit 0\n");
    assert!(out.lines().any(|l| l == "bye"), "stdout: {out}");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn exit_trap_sees_last_status() {
    let (out, _err, _) = run("trap 'echo dollar-q=$?' EXIT\nfalse\nexit\n");
    assert!(out.lines().any(|l| l == "dollar-q=1"), "stdout: {out}");
}

#[test]
fn exit_trap_fires_on_eof() {
    // Script ends without explicit `exit`. EOF should still fire EXIT.
    let (out, _err, _) = run("trap 'echo bye' EXIT\n");
    assert!(out.lines().any(|l| l == "bye"), "stdout: {out}");
}

#[test]
fn exit_trap_fires_only_once() {
    // Recursive exit from within the action should NOT re-fire.
    let (out, _err, _) = run("trap 'echo bye; exit 0' EXIT\nexit 1\n");
    let bye_count = out.lines().filter(|l| **l == *"bye").count();
    assert_eq!(bye_count, 1, "stdout: {out}");
}

#[test]
fn exit_trap_cleared_in_subshell() {
    // Parent's EXIT fires only once when the parent exits. Subshell
    // does NOT fire it again.
    let (out, _err, _) = run("trap 'echo parent-bye' EXIT\n(echo child)\nexit\n");
    let bye_count = out.lines().filter(|l| **l == *"parent-bye").count();
    assert_eq!(bye_count, 1, "stdout: {out}");
    assert!(out.lines().any(|l| l == "child"), "stdout: {out}");
}

#[test]
fn trap_dash_resets_exit() {
    // Set, then reset. EXIT trap should NOT fire.
    let (out, _err, _) = run("trap 'echo bye' EXIT\ntrap - EXIT\nexit 0\n");
    assert!(!out.lines().any(|l| l == "bye"), "stdout: {out}");
}

#[test]
fn trap_empty_action_ignores_exit() {
    // Empty action = ignore. EXIT does not run anything.
    let (out, _err, _) = run("trap '' EXIT\nexit 0\n");
    // No specific output to assert non-presence of — but exit must succeed.
    assert!(!out.contains("bye"));
}

#[test]
fn trap_p_output_format() {
    let (out, _err, _) = run("trap 'echo bye' EXIT\ntrap -p\nexit\n");
    // The `trap -p` output should appear before the EXIT action runs.
    assert!(
        out.lines().any(|l| l == "trap -- 'echo bye' EXIT"),
        "stdout: {out}"
    );
}

#[test]
fn trap_l_lists_signals() {
    let (out, _err, _) = run("trap -l\nexit\n");
    assert!(out.contains("2) SIGINT"), "stdout: {out}");
    assert!(out.contains("15) SIGTERM"), "stdout: {out}");
}

#[test]
fn trap_kill_accepted_silently_and_listed() {
    // bash does NOT reject `trap … KILL`: it silently accepts (no error) and
    // stores the disposition (visible via `trap -p`), though it never fires.
    let (_out, err, _) = run("trap 'echo nope' KILL\nexit 0\n");
    assert!(
        !err.contains("cannot trap"),
        "should not error; stderr: {err}"
    );
    assert!(err.is_empty(), "no stderr expected; got: {err}");
    let (out, _err, _) = run("trap 'echo nope' KILL\ntrap -p KILL\n");
    assert!(
        out.contains("echo nope") && out.contains("KILL"),
        "trap -p should list the KILL disposition; stdout: {out}"
    );
}

#[test]
fn trap_unknown_signal_errors_exit_1() {
    let (_out, err, _) = run("trap 'echo nope' NOPE\nexit 0\n");
    assert!(
        err.contains("invalid signal specification"),
        "stderr: {err}"
    );
}

#[test]
fn sigint_trap_fires_action() {
    // Spawn huck with a script that installs a SIGINT trap and then
    // sleeps. From the test, wait briefly for the trap to register +
    // the sleep to start, then send SIGINT to the child. After the
    // sleep returns (signal interrupts it), the trap action runs and
    // huck continues to the `exit`.
    let (mut child, pid) = spawn("trap 'echo caught' INT\nsleep 2\nexit\n");
    // Drop stdin so huck reads to EOF and runs the script body.
    drop(child.stdin.take());
    // Give huck ~200ms to parse the trap line + start sleeping.
    std::thread::sleep(Duration::from_millis(200));
    send_signal(pid, libc::SIGINT);
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("caught"), "stdout: {stdout}");
}

#[test]
fn trap_in_function_persists_after_return() {
    // trap is shell-global, not function-local.
    let script = "f() { trap 'echo bye' EXIT; }\nf\nexit 0\n";
    let (out, _err, _) = run(script);
    assert!(out.lines().any(|l| l == "bye"), "stdout: {out}");
}

#[test]
fn a_forking_err_trap_action_does_not_disturb_the_surrounding_status() {
    // The ERR action's own `$?` (7 here) must not leak into the status the
    // script observes after the failing command — that stays 42.
    //
    // ⚠️ This lives here, not beside `run_trap_action`'s unit tests, because the
    // action FORKS: `(exit 7)` is a subshell, and so is the `(exit 42)` that
    // triggers it. huck runs subshells by forking without exec, which is only
    // safe in a single-threaded process, so `assert_single_threaded_fork()`
    // panics if any other thread is executing shell code. In the shared lib test
    // binary ~2000 tests run concurrently and that guard fires (macOS loses the
    // race every time; Linux usually wins it) — see #747. Each test here spawns
    // its OWN huck process, which is single-threaded, so forking is safe.
    //
    // The unit test keeps the same two assertions with a non-forking action;
    // this row is what preserves coverage of a SUBSHELL action end to end.
    // Verified byte-identical against bash 5.2.21.
    let script = "trap \"(exit 7)\" ERR\n(exit 42)\necho \"after=$?\"\n";
    let (out, _err, _) = run(script);
    assert!(
        out.lines().any(|l| l == "after=42"),
        "the trap action's status leaked into the surrounding $?; stdout: {out}"
    );
}
