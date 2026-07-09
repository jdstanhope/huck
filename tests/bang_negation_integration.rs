//! Integration tests for v85 `!` pipeline negation.
use std::io::Write;
use std::process::{Command, Stdio};

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
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
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into(),
        o.status.code().unwrap_or(-1),
    )
}

#[test]
fn bang_basic() {
    assert_eq!(run("! false\necho $?\n").0, "0\n");
    assert_eq!(run("! true\necho $?\n").0, "1\n");
}

#[test]
fn bang_in_if_condition() {
    assert_eq!(run("if ! false; then echo yes; fi\n").0, "yes\n");
    assert_eq!(
        run("if ! true; then echo yes; else echo no; fi\n").0,
        "no\n"
    );
}

#[test]
fn bang_in_while_condition() {
    // `while ! true` never enters the loop.
    assert_eq!(
        run("while ! true; do echo x; done\necho done\n").0,
        "done\n"
    );
}

#[test]
fn bang_with_and() {
    assert_eq!(run("! false && echo ran\n").0, "ran\n");
}

#[test]
fn bang_pipeline_status_and_pipestatus() {
    // negate the whole pipeline; PIPESTATUS stays raw.
    assert_eq!(
        run("! false | true\necho \"$? ${PIPESTATUS[@]}\"\n").0,
        "1 1 0\n"
    );
}

#[test]
fn bang_exempt_from_errexit() {
    // set -e; ! true (result 1) must NOT exit the shell.
    assert_eq!(run("set -e\n! true\necho survived\n").0, "survived\n");
}

#[test]
fn bang_with_pipefail() {
    assert_eq!(run("set -o pipefail\n! false | true\necho $?\n").0, "0\n");
}

#[test]
fn bang_before_compound() {
    assert_eq!(run("! { false; }\necho $?\n").0, "0\n");
    assert_eq!(run("! (exit 3)\necho $?\n").0, "0\n");
}

#[test]
fn double_bang_cancels() {
    assert_eq!(run("! ! false\necho $?\n").0, "1\n");
}
