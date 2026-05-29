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
fn disown_prefix_match() {
    let script = "sleep 30 >/dev/null 2>&1 &\ndisown %sleep\necho $?\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "0"),
        "expected status 0 in: {:?}",
        out
    );
}

#[test]
fn disown_ambiguous_errors() {
    let script = "sleep 30 >/dev/null 2>&1 &\nsleep 60 >/dev/null 2>&1 &\ndisown %sleep\nrc=$?\necho rc=$rc\nexit\n";
    let (out, err) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "rc=1"),
        "expected rc=1 in stdout: {:?}; stderr: {:?}",
        out,
        err
    );
    assert!(
        err.contains("ambiguous"),
        "expected stderr to contain 'ambiguous': {:?}",
        err
    );
}

#[test]
fn jobs_substring_filter_via_spec() {
    let script = "sleep 30 >/dev/null 2>&1 &\njobs %?sleep\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.contains("sleep"),
        "expected stdout to contain 'sleep': {:?}",
        out
    );
}
