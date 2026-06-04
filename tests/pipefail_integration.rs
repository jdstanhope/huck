//! Integration tests for v83 set -o pipefail + $PIPESTATUS (M-50).
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
fn pipestatus_after_multistage() {
    // All three index forms read from the same pipeline result in one
    // simple command (so the read itself doesn't clobber PIPESTATUS first).
    let (out, _) = run(
        "true | false | true\necho \"${PIPESTATUS[@]} | [1]=${PIPESTATUS[1]} n=${#PIPESTATUS[@]}\"\n",
    );
    assert_eq!(out, "0 1 0 | [1]=1 n=3\n");
}

#[test]
fn pipefail_off_default_uses_last_stage() {
    assert_eq!(run("false | true\necho rc=$?\n").0, "rc=0\n");
}

#[test]
fn pipefail_on_rightmost_nonzero() {
    assert_eq!(run("set -o pipefail\nfalse | true\necho rc=$?\n").0, "rc=1\n");
    assert_eq!(
        run("set -o pipefail\n(exit 2) | (exit 3)\necho rc=$?\n").0,
        "rc=3\n"
    );
    assert_eq!(run("set -o pipefail\ntrue | true\necho rc=$?\n").0, "rc=0\n");
}

#[test]
fn pipestatus_after_simple_command() {
    assert_eq!(run("false\necho \"${PIPESTATUS[@]}\"\n").0, "1\n");
    assert_eq!(run("true\necho \"${PIPESTATUS[@]}\"\n").0, "0\n");
}

#[test]
fn pipestatus_compound_transparency() {
    // if with false condition → PIPESTATUS reflects the condition (1), not the if (0).
    assert_eq!(
        run("if false; then :; fi\necho \"${PIPESTATUS[@]} rc=$?\"\n").0,
        "1 rc=0\n"
    );
    // for loop body's last pipeline.
    assert_eq!(
        run("for i in 1; do true | false; done\necho \"${PIPESTATUS[@]}\"\n").0,
        "0 1\n"
    );
    // brace group is transparent.
    assert_eq!(
        run("{ true | false; }\necho \"${PIPESTATUS[@]}\"\n").0,
        "0 1\n"
    );
}

#[test]
fn pipestatus_subshell_is_one_element() {
    assert_eq!(run("(true | false)\necho \"${PIPESTATUS[@]}\"\n").0, "1\n");
}

#[test]
fn pipestatus_function_is_opaque() {
    assert_eq!(
        run("f() { true | false; }\nf\necho \"${PIPESTATUS[@]}\"\n").0,
        "1\n"
    );
    assert_eq!(
        run("g() { return 5; }\ntrue | false | true\ng\necho \"${PIPESTATUS[@]}\"\n").0,
        "5\n"
    );
}
