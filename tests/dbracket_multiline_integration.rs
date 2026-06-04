//! Integration tests for v87 multi-line [[ ]] continuation + test operators (M-14a).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Runs `script` through huck on stdin; returns (stdout, exit_code).
fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn multiline_break_before_close() {
    // `]]` on the next line.
    assert_eq!(run("[[ -f /etc/passwd\n]] && echo yes\n").0, "yes\n");
}

#[test]
fn multiline_break_after_and() {
    assert_eq!(run("[[ -f /etc/passwd &&\n   -f /etc/hosts ]] && echo both\n").0, "both\n");
}

#[test]
fn multiline_break_after_open() {
    assert_eq!(run("[[\n  -f /etc/passwd ]] && echo opened\n").0, "opened\n");
}

#[test]
fn multiline_break_after_operand() {
    assert_eq!(run("[[ abc\n== abc ]] && echo eq\n").0, "eq\n");
}

#[test]
fn singleline_still_works() {
    assert_eq!(run("[[ -f /etc/passwd ]] && echo ok\n").0, "ok\n");
}

#[test]
fn bare_double_bracket_token_is_literal_arg() {
    // `echo [[` must NOT hang waiting for `]]`; prints the literal.
    assert_eq!(run("echo [[\n").0, "[[\n");
}
