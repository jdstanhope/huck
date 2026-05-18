use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_in_cwd(cwd: &std::path::Path, script: &str) -> String {
    let mut child = Command::new(huck_binary())
        .current_dir(cwd)
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn touch(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), b"").unwrap();
}

#[test]
fn echo_star_matches_cwd_files_sorted() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    touch(tmp.path(), "b.txt");
    let out = run_in_cwd(tmp.path(), "echo *.txt\nexit\n");
    assert!(out.lines().any(|l| l == "a.txt b.txt"), "stdout: {out}");
}

#[test]
fn echo_quoted_star_is_literal() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    let out = run_in_cwd(tmp.path(), "echo \"*.txt\"\nexit\n");
    assert!(out.lines().any(|l| l == "*.txt"), "stdout: {out}");
}

#[test]
fn echo_no_match_passes_pattern_literally() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_in_cwd(tmp.path(), "echo *.nope\nexit\n");
    assert!(out.lines().any(|l| l == "*.nope"), "stdout: {out}");
}

#[test]
fn echo_bracket_class() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    touch(tmp.path(), "b.txt");
    touch(tmp.path(), "c.txt");
    let out = run_in_cwd(tmp.path(), "echo [ab].txt\nexit\n");
    assert!(out.lines().any(|l| l == "a.txt b.txt"), "stdout: {out}");
}

#[test]
fn echo_tilde_glob_combo() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "x.dat");
    touch(tmp.path(), "y.dat");
    // Set HOME via env on the child so ~/*.dat expands to the temp dir.
    let mut child = Command::new(huck_binary())
        .env("HOME", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"echo ~/*.dat\nexit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    let expected_a = format!("{}/x.dat", tmp.path().display());
    let expected_b = format!("{}/y.dat", tmp.path().display());
    let expected = format!("{expected_a} {expected_b}");
    assert!(s.lines().any(|l| l == expected), "stdout: {s}");
}
