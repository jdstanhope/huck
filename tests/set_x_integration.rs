//! v103: set -x (xtrace).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Returns (stdout, stderr, exit_code).
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn traces_simple_command() {
    let (so, se, _) = run("set -x\necho hi\n");
    assert_eq!(so, "hi\n");
    assert!(se.contains("+ echo hi"), "stderr={se:?}");
}

#[test]
fn traces_expanded_form() {
    let (so, se, _) = run("x=hi\nset -x\necho \"$x\" a\n");
    assert_eq!(so, "hi a\n");
    assert!(se.contains("+ echo hi a"), "stderr={se:?}");
}

#[test]
fn enabling_line_not_traced_disabling_is() {
    let (_so, se, _) = run("set -x\necho a\nset +x\necho b\n");
    assert!(se.contains("+ echo a"), "stderr={se:?}");
    assert!(se.contains("+ set +x"), "stderr={se:?}");
    assert!(
        !se.contains("+ echo b"),
        "echo b should NOT be traced: {se:?}"
    );
}

#[test]
fn traces_inside_function() {
    let (_so, se, _) = run("f() { echo in; }\nset -x\nf\n");
    assert!(se.contains("+ f"), "stderr={se:?}");
    assert!(se.contains("+ echo in"), "stderr={se:?}");
}

#[test]
fn dollar_dash_has_x() {
    let (so, _se, _) = run("set -x\ncase \"$-\" in *x*) echo on;; *) echo off;; esac\n");
    assert_eq!(so, "on\n");
}

#[test]
fn xtrace_to_stderr_not_captured() {
    let (so, _se, _) = run("r=$(set -x; echo cap)\necho \"[$r]\"\n");
    assert_eq!(so, "[cap]\n");
}

#[test]
fn set_o_xtrace_form() {
    let (_so, se, _) = run("set -o xtrace\necho hi\n");
    assert!(se.contains("+ echo hi"), "stderr={se:?}");
}
