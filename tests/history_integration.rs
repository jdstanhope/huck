use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with the given stdin script and an isolated HISTFILE.
fn run_with_histfile(script: &str, histfile: &std::path::Path) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .env("HISTFILE", histfile)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn bang_bang_reruns_previous_command() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile("echo hello\n!!\nexit\n", &hf);
    let count = out.lines().filter(|l| *l == "hello").count();
    assert!(count >= 2, "expected 'hello' at least twice, stdout: {out}");
}

#[test]
fn bang_dollar_substitutes_last_argument() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile("echo alpha beta\necho !$\nexit\n", &hf);
    assert!(out.lines().any(|l| l == "beta"), "stdout: {out}");
}

#[test]
fn quick_substitution_replaces_text() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile("echo hello\n^hello^goodbye^\nexit\n", &hf);
    assert!(out.lines().any(|l| l == "goodbye"), "stdout: {out}");
}

#[test]
fn failed_expansion_writes_error_and_does_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (_, err) = run_with_histfile("!nonexistent\nexit\n", &hf);
    assert!(err.contains("event not found"), "stderr: {err}");
}

#[test]
fn history_builtin_lists_commands() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile("echo aaa\necho bbb\nhistory\nexit\n", &hf);
    assert!(out.contains("echo aaa"), "stdout: {out}");
    assert!(out.contains("echo bbb"), "stdout: {out}");
}

#[test]
fn history_dash_c_clears() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    let (out, _) = run_with_histfile(
        "echo keep\nhistory -c\nhistory\nexit\n",
        &hf,
    );
    // After clearing with history -c, the only entry in history is the 'history' command itself
    // Check that 'echo keep' is NOT in the output
    assert!(!out.contains("echo keep"), "history should not contain 'echo keep' after clear, stdout: {out}");
}

#[test]
fn history_persists_across_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("h");
    run_with_histfile("echo first\necho second\nexit\n", &hf);
    let (out, _) = run_with_histfile("history\nexit\n", &hf);
    assert!(out.contains("echo first"), "stdout: {out}");
    assert!(out.contains("echo second"), "stdout: {out}");
}
