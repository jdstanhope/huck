use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn integer_assign_evaluates() {
    let (out, _, _) = run_capture("declare -i X=2+3\necho $X\nexit\n");
    assert!(out.lines().any(|l| l == "5"), "stdout: {out:?}");
}

#[test]
fn integer_reassign_evaluates() {
    let (out, _, _) = run_capture("declare -i X\nX=10*5\necho $X\nexit\n");
    assert!(out.lines().any(|l| l == "50"), "stdout: {out:?}");
}

#[test]
fn integer_garbage_becomes_zero() {
    let (out, _, _) = run_capture("declare -i X=abc\necho $X\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out:?}");
}

#[test]
fn integer_p_format() {
    let (out, _, _) = run_capture("declare -i X=42\ndeclare -p X\nexit\n");
    assert!(
        out.lines().any(|l| l == "declare -i X=\"42\""),
        "stdout: {out:?}",
    );
}

#[test]
fn plus_i_unmarks() {
    let (out, _, _) = run_capture("declare -i X=10\ndeclare +i X\nX=2+3\necho $X\nexit\n");
    // After +i, X=2+3 stores literally.
    assert!(out.lines().any(|l| l == "2+3"), "stdout: {out:?}");
}

#[test]
fn integer_in_for_loop() {
    let (out, _, _) = run_capture("declare -i X\nfor X in 2+3 7-1; do echo $X; done\nexit\n");
    let collected: Vec<&str> = out.lines().take(2).collect();
    assert_eq!(collected, vec!["5", "6"], "stdout: {out:?}");
}
