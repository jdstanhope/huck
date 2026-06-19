use std::io::Write;
use std::process::{Command, Stdio};

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

#[test]
fn trap_fires_on_newly_supported_signal() {
    // SEGV was un-trappable (panic) before register_unchecked. Now `trap … SEGV`
    // installs a handler that fires when the signal is delivered via `kill -SEGV`.
    let (out, _err, _) = run("trap 'echo HIT' SEGV\nkill -SEGV $$\nexit\n");
    assert!(out.contains("HIT"), "stdout: {out}");
}

#[test]
fn kill_l_round_trips_new_signal() {
    // name -> number
    let (out, _err, _) = run("kill -l ABRT\nexit\n");
    assert_eq!(out.trim(), libc::SIGABRT.to_string(), "stdout: {out}");

    // number -> name
    let script = format!("kill -l {}\nexit\n", libc::SIGABRT);
    let (out, _err, _) = run(&script);
    assert_eq!(out.trim(), "ABRT", "stdout: {out}");
}
