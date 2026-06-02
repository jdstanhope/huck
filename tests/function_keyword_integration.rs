//! Integration tests for v77 `function NAME { ... }` keyword form.
//! Drives the `huck` binary via stdin and asserts on stdout/exit code.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn keyword_form_define_and_call() {
    let (out, _, code) = run_huck("function greet { echo hello; }\ngreet\n");
    assert_eq!(code, 0);
    assert_eq!(out, "hello\n");
}

#[test]
fn keyword_form_with_optional_parens() {
    let (out, _, code) = run_huck("function greet() { echo hi; }\ngreet\n");
    assert_eq!(code, 0);
    assert_eq!(out, "hi\n");
}

#[test]
fn keyword_form_positional_args_propagate() {
    let script = r#"function f { echo "$1-$2"; }
f alpha beta
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "alpha-beta\n");
}

#[test]
fn keyword_form_and_posix_form_are_equivalent() {
    let script = r#"function kf { echo "via $1"; }
pf() { echo "via $1"; }
kf keyword
pf posix
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "via keyword\nvia posix\n");
}

#[test]
fn keyword_form_redefine_via_posix_latest_wins() {
    let script = r#"function f { echo first; }
f() { echo second; }
f
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "second\n");
}

#[test]
fn keyword_form_subshell_body() {
    let script = r#"function f() ( echo subshell-body )
f
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "subshell-body\n");
}
