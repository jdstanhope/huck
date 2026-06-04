//! Integration tests for v90 extglob string matching (M-84).
use std::io::Write;
use std::process::{Command, Stdio};
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
fn run(script: &str) -> (String, i32) {
    let mut c = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    c.stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let o = c.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

#[test]
fn param_expansion_extglob() {
    assert_eq!(run("shopt -s extglob\nv=aaab\necho \"${v##+(a)}\"\n").0, "b\n");
    assert_eq!(
        run("shopt -s extglob\nv=foobarbar\necho \"${v%%+(bar)}\"\n").0,
        "foo\n"
    );
    assert_eq!(
        run("shopt -s extglob\nv=abcabc\necho \"${v/+(abc)/X}\"\n").0,
        "X\n"
    );
}

#[test]
fn param_expansion_extglob_off_is_literal() {
    // extglob off: `+(a)` is a literal pattern, no strip (matches bash).
    assert_eq!(run("v=aaab\necho \"${v##+(a)}\"\n").0, "aaab\n");
}
