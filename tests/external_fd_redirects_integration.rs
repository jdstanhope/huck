//! End-to-end tests for v156 task 4: arbitrary-fd redirections on EXTERNAL
//! (forked) commands. Each test compares `huck -c <script>` to `bash -c
//! <script>` for byte-identical stdout, exercising fd>2 opens, `<&` dup-in,
//! `N>&-` close, and a full fd swap — all replayed in the child's pre_exec.

use std::process::Command;

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Run `prog -c script` and return (stdout, exit_code).
fn run_c(prog: &str, script: &str) -> (String, i32) {
    let out = Command::new(prog)
        .arg("-c")
        .arg(script)
        .output()
        .expect("spawn");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Assert huck's stdout matches bash's for the same script.
fn assert_matches_bash(script: &str) {
    let (bash_out, _bash_rc) = run_c("bash", script);
    let (huck_out, _huck_rc) = run_c(&huck_binary(), script);
    assert_eq!(
        huck_out, bash_out,
        "stdout differs for script: {script}\n bash: {bash_out:?}\n huck: {huck_out:?}"
    );
}

#[test]
fn fd_swap_external() {
    // Classic stdout/stderr swap via a scratch fd 3, then discard the swapped
    // stderr. bash prints only `O`.
    assert_matches_bash("sh -c 'echo O; echo E >&2' 3>&1 1>&2 2>&3 3>&- 2>/dev/null");
}

#[test]
fn external_writes_via_parent_opened_fd3() {
    // fd 3 is opened by the shell for this external command; the child inherits
    // it and `echo hi >&3` lands in the file.
    let tmp = format!("/tmp/huck_extfd3_{}", std::process::id());
    let script = format!("sh -c 'echo hi >&3' 3>{tmp}; cat {tmp}; rm -f {tmp}");
    assert_matches_bash(&script);
}

#[test]
fn unused_extra_fd_creates_empty_file() {
    // fd 5 is opened for an external that ignores it: the file is created empty,
    // matching bash. Only stdout (fd 1) carries `x`.
    let f = format!("/tmp/huck_f_{}", std::process::id());
    let g = format!("/tmp/huck_g_{}", std::process::id());
    let script = format!(
        "printf x >{f} 5>{g}; printf 'F=%s G=%s' \"$(cat {f})\" \"$(wc -c <{g})\"; rm -f {f} {g}"
    );
    assert_matches_bash(&script);
}

#[test]
fn dup_in_from_herestring_external() {
    // `<&0` duplicates stdin (here fed by a here-string) onto fd 0 for `cat`.
    assert_matches_bash("cat <&0 <<< piped");
}

#[test]
fn close_extra_fd_external() {
    // `3>&-` closing an fd the child never opened is a no-op; the command runs.
    assert_matches_bash("sh -c 'echo ok' 3>&-");
}

#[test]
fn append_extra_fd_external() {
    // Append to a high fd then read it back.
    let tmp = format!("/tmp/huck_app_{}", std::process::id());
    let script =
        format!("printf 'a\\n' >{tmp}; sh -c 'echo b >&4' 4>>{tmp}; cat {tmp}; rm -f {tmp}");
    assert_matches_bash(&script);
}

#[test]
fn pipeline_stage_high_fd_redirect_not_clobbered_by_pipe_close() {
    // Regression test for Fix B: a pipeline stage with an fd>2 file redirect
    // must not have that fd silently closed by the fds_to_close pre_exec.
    // We redirect fd 9 in a pipeline non-final stage and verify the file
    // contains the expected output (the redirect survived exec).
    let tmp = format!("/tmp/hk_pf_{}", std::process::id());
    // Use fd 9 (high, unlikely to be a pipe fd) to reduce collision chance.
    // The inner sh writes "hi" to &9, which the outer shell mapped to $tmp.
    let script =
        format!("sh -c 'echo hi >&9' 9>{tmp} | cat; echo \"file=$(cat {tmp})\"; rm -f {tmp}");
    assert_matches_bash(&script);
}
