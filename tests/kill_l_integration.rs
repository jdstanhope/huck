use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
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

#[test]
fn kill_l_bare_lists_signals() {
    // `kill -l` then exit; stdout should contain both TERM and KILL.
    let (out, _) = run("kill -l\nexit\n");
    assert!(out.contains("TERM"), "stdout missing TERM: {:?}", out);
    assert!(out.contains("KILL"), "stdout missing KILL: {:?}", out);
}

#[test]
fn kill_l_name_to_number() {
    // `kill -l TERM` should print SIGTERM's number on its own line.
    let (out, _) = run("kill -l TERM\nexit\n");
    let expected = format!("{}", libc::SIGTERM);
    assert!(
        out.lines().any(|l| l == expected),
        "expected line {expected:?} in stdout: {out:?}"
    );
}
