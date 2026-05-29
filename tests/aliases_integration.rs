use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .env("HUCK_EXPAND_ALIASES", "1")
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
fn alias_expansion_via_repl() {
    let script = "alias ll='echo HELLO'\nll\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "HELLO"),
        "expected HELLO line in: {:?}",
        out
    );
}

#[test]
fn unalias_removes_expansion() {
    let script =
        "alias ll='echo HELLO'\nunalias ll\nll\nrc=$?\necho rc=$rc\nexit\n";
    let (out, err) = run_capture(script);
    let rc_line = out.lines().find(|l| l.starts_with("rc="));
    assert!(rc_line.is_some(), "no rc= line in: {:?}", out);
    let rc = rc_line.unwrap();
    assert_ne!(rc, "rc=0", "expected non-zero rc, got {rc}; stderr {:?}", err);
}

#[test]
fn recursive_alias_chain() {
    let script = "alias l='ll'\nalias ll='echo INNER'\nl\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "INNER"),
        "expected INNER line in: {:?}",
        out
    );
}
