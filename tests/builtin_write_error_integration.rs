//! v308 (#186 #190 #191): a builtin writing to a real fd that cannot be written
//! reports bash's `<name>: write error: <strerror>` + rc 1, and its failed
//! output reaches NOTHING — least of all the fd 1 that the redirect scope
//! restores afterwards.
//!
//! `exec 3</etc/hostname` gives a genuinely read-only fd (the kernel returns
//! EBADF for a write); `/dev/full` gives ENOSPC. Externals are already correct,
//! so these all exercise the in-process builtin path.

use std::process::Command;

fn huck() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run `huck -c script`, returning (stdout, stderr, rc).
fn run(script: &str) -> (String, String, i32) {
    let out = Command::new(huck())
        .arg("-c")
        .arg(script)
        .output()
        .expect("spawn huck");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn echo_to_read_only_fd_reports_and_fails() {
    let (out, err, rc) = run("exec 3</etc/hostname; echo x >&3");
    assert!(
        err.contains("echo: write error: Bad file descriptor"),
        "stderr was: {err:?}"
    );
    assert_eq!(rc, 1, "exit status");
    assert_eq!(out, "", "nothing may reach the real stdout");
}

#[test]
fn printf_to_read_only_fd_reports_and_fails() {
    let (_, err, rc) = run("exec 3</etc/hostname; printf x >&3");
    assert!(
        err.contains("printf: write error: Bad file descriptor"),
        "stderr was: {err:?}"
    );
    assert_eq!(rc, 1);
}

/// `declare` discards its own write Result (one of ~82 such sites), so this
/// only works if the WRITER records the error rather than the builtin checking.
#[test]
fn declare_to_read_only_fd_reports_and_fails() {
    let (_, err, rc) = run("exec 3</etc/hostname; x=1; declare -p x >&3");
    assert!(
        err.contains("declare: write error: Bad file descriptor"),
        "stderr was: {err:?}"
    );
    assert_eq!(rc, 1);
}

/// #191: the payload must never reach the restored fd 1. All four shapes leaked
/// before this change.
#[test]
fn failed_output_never_leaks_to_the_restored_fd1() {
    for script in [
        "echo x > /dev/full",
        "echo -n x > /dev/full",
        "printf 'x' > /dev/full",
        "x=1; declare -p x > /dev/full",
    ] {
        let (out, _, rc) = run(script);
        assert_eq!(out, "", "payload leaked to the real stdout for: {script}");
        assert_eq!(rc, 1, "exit status for: {script}");
    }
}

/// #190: the wording must not depend on a trailing newline. Before this change
/// the newline chose the reporter, and the two disagreed.
#[test]
fn wording_is_identical_with_and_without_a_trailing_newline() {
    for script in [
        "printf 'x\\n' > /dev/full",
        "printf 'x' > /dev/full",
        "echo x > /dev/full",
        "echo -n x > /dev/full",
    ] {
        let (_, err, rc) = run(script);
        assert!(
            err.contains("write error: No space left on device"),
            "wrong wording for {script:?}: {err:?}"
        );
        assert!(
            !err.contains("os error"),
            "raw io::Error leaked into the message for {script:?}: {err:?}"
        );
        assert_eq!(rc, 1);
    }
}

/// bash is SILENT when no bytes are written, even to a broken fd.
#[test]
fn zero_byte_output_is_silent_and_succeeds() {
    for script in [
        "exec 3</etc/hostname; echo -n '' >&3",
        "exec 3</etc/hostname; printf '' >&3",
        "exec 3</etc/hostname; : >&3",
        "exec 3</etc/hostname; true >&3",
        "exec 3</etc/hostname; jobs >&3",
        "exec 3</etc/hostname; cd /tmp >&3",
    ] {
        let (_, err, rc) = run(script);
        assert!(!err.contains("write error"), "must be silent: {script}");
        assert_eq!(rc, 0, "exit status for: {script}");
    }
}

/// A writable fd must not be reported (guards against false positives).
#[test]
fn writable_fd_is_not_reported() {
    // PID-unique path + cleanup: a fixed `/tmp` path left behind by a prior
    // run (possibly owned by a different UID on a shared `/tmp`) makes
    // `exec 3<>` fail with EACCES, which fails this test for a reason
    // unrelated to the thing it's testing.
    let path = format!("/tmp/huck-v308-rw.{}.txt", std::process::id());
    let (_, err, rc) = run(&format!("exec 3<>{path}; echo x >&3"));
    let _ = std::fs::remove_file(&path);
    assert!(!err.contains("write error"), "stderr was: {err:?}");
    assert_eq!(rc, 0);
}
