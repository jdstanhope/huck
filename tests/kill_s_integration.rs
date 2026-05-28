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
fn kill_s_invalid_name_errors_status_1() {
    // `kill -s BOGUS 99999` exits 1 before any send is attempted.
    let (out, _) = run("kill -s BOGUS 99999\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {:?}", out);
}

#[test]
fn kill_s_missing_arg_errors_status_2() {
    let (out, _) = run("kill -s\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "2"), "stdout: {:?}", out);
}

#[test]
fn kill_n_invalid_number_errors_status_1() {
    // 99 isn't in killable_signals(); parse error before send.
    let (out, _) = run("kill -n 99 99999\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {:?}", out);
}
