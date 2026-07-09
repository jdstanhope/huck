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
fn if_with_and_combinator() {
    let (out, _, _) = run_capture("if [ -n \"a\" -a -n \"b\" ]; then echo Y; fi\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn if_with_or_combinator() {
    let (out, _, _) = run_capture("if [ -z \"\" -o -n \"x\" ]; then echo Y; fi\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn nested_parens_in_if() {
    let (out, _, _) = run_capture("if [ \\( -n a -o -n b \\) -a -n c ]; then echo Y; fi\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn negated_combinator_in_if() {
    let (out, _, _) = run_capture("if [ ! \\( -z a -o -z b \\) ]; then echo Y; fi\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn bracket_form_with_combinator() {
    let (out, _, _) = run_capture("[ -n a -a -n b ] && echo Y\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "got: {out:?}");
}

#[test]
fn unbalanced_paren_produces_non_zero_exit() {
    let (out, _, _) = run_capture("[ \\( -n a ]\necho rc=$?\nexit\n");
    let rc_line = out.lines().find(|l| l.starts_with("rc=")).unwrap_or("rc=?");
    assert_ne!(rc_line, "rc=0", "expected non-zero rc, got: {rc_line}");
}
