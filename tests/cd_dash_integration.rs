use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Path that `cd <p>` will end up printing/storing. Like bash, huck keeps the
/// *logical* PWD — the path you cd'd through, NOT its symlink-resolved form — so
/// `cd /tmp; pwd` prints `/tmp` on both Linux and macOS (verified: identical to
/// bash on macOS, where `/tmp` → `/private/tmp` physically but stays `/tmp`
/// logically). The assertions therefore compare against the literal path.
fn logical_pwd(p: &str) -> String {
    p.to_string()
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
fn cd_dash_returns_to_previous_directory() {
    let script = "cd /tmp\ncd /var\ncd -\npwd\nexit\n";
    let (out, _err, _) = run_capture(script);
    // The "cd -" line itself prints the new PWD, then `pwd` prints it again.
    let tmp = logical_pwd("/tmp");
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines.contains(&tmp.as_str()),
        "expected {tmp} in stdout, got: {out:?}"
    );
    let tmp_lines = lines.iter().filter(|l| **l == tmp).count();
    assert_eq!(tmp_lines, 2, "expected {tmp} printed twice, got: {out:?}");
}

#[test]
fn cd_dash_prints_new_pwd() {
    let script = "cd /tmp\ncd /var\ncd -\nexit\n";
    let (out, _err, _) = run_capture(script);
    let tmp = logical_pwd("/tmp");
    assert!(
        out.lines().any(|l| l == tmp),
        "expected cd - to print {tmp}, got stdout: {out:?}"
    );
}

#[test]
fn cd_dash_errors_when_oldpwd_unset() {
    let script = "unset OLDPWD\ncd -\necho rc=$?\nexit\n";
    let (out, err, _) = run_capture(script);
    assert!(
        err.contains("OLDPWD not set"),
        "expected OLDPWD error, got stderr: {err:?}"
    );
    assert!(
        out.lines().any(|l| l == "rc=1"),
        "expected rc=1, got stdout: {out:?}"
    );
}

#[test]
fn cd_dash_swaps_pwd_and_oldpwd() {
    let script = "cd /tmp\ncd /var\necho pre PWD=$PWD OLDPWD=$OLDPWD\ncd -\necho post PWD=$PWD OLDPWD=$OLDPWD\nexit\n";
    let (out, _err, _) = run_capture(script);
    let tmp = logical_pwd("/tmp");
    let var = logical_pwd("/var");
    let pre = format!("pre PWD={var} OLDPWD={tmp}");
    let post = format!("post PWD={tmp} OLDPWD={var}");
    assert!(out.lines().any(|l| l == pre), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == post), "stdout: {out:?}");
}
