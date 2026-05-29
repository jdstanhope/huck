use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
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
fn bg_and_chain_runs_to_completion() {
    let script = "echo A && echo B &\nwait\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "A"),
        "expected line A in: {:?}",
        out
    );
    assert!(
        out.lines().any(|l| l == "B"),
        "expected line B in: {:?}",
        out
    );
}

#[test]
fn bg_semi_chain_runs_both() {
    let script = "echo X ; echo Y &\nwait\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "X"),
        "expected line X in: {:?}",
        out
    );
    assert!(
        out.lines().any(|l| l == "Y"),
        "expected line Y in: {:?}",
        out
    );
}

#[test]
fn bg_chain_short_circuits() {
    let script = "false && echo SKIP &\nwait\necho DONE\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "DONE"),
        "expected DONE line in: {:?}",
        out
    );
    assert!(
        !out.lines().any(|l| l == "SKIP"),
        "SKIP should NOT appear (short-circuit): {:?}",
        out
    );
}
