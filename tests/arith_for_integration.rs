//! Integration tests for v78 C-style for-loop and standalone arith
//! command. Drives the `huck` binary via stdin and asserts on stdout
//! and exit code.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
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
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn standalone_arith_assignment_persists() {
    let (out, _, code) = run_huck("((x=5))\necho $x\n");
    assert_eq!(code, 0);
    assert_eq!(out, "5\n");
}

#[test]
fn arith_for_counter_prints_each_value() {
    let (out, _, code) = run_huck("for ((i=0;i<5;i++)) do printf '%d ' $i; done\n");
    assert_eq!(code, 0);
    assert_eq!(out, "0 1 2 3 4 ");
}

#[test]
fn arith_for_infinite_with_break_terminates() {
    let (out, _, code) = run_huck("for ((;;)) do break; done\necho ok\n");
    assert_eq!(code, 0);
    assert_eq!(out, "ok\n");
}

#[test]
fn arith_command_in_if_condition() {
    let (out, _, code) = run_huck("if ((5 > 3)); then echo positive; fi\n");
    assert_eq!(code, 0);
    assert_eq!(out, "positive\n");
}

#[test]
fn arith_for_nested() {
    let script = "for ((i=0;i<2;i++)) do for ((j=0;j<2;j++)) do printf '%d%d ' $i $j; done; done\n";
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "00 01 10 11 ");
}

#[test]
fn arith_for_continue_skips_to_step() {
    let script =
        "for ((i=0;i<5;i++)) do if [ $i -eq 2 ]; then continue; fi; printf '%d ' $i; done\n";
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "0 1 3 4 ");
}

#[test]
fn double_paren_no_space_is_arith_not_subshell() {
    // Pre-v78: `((5+5))` parsed as nested subshell (`( (5+5) )`), which
    // would try to run `5+5` as a command and error. Post-v78: arith,
    // exits 0 because the result is non-zero.
    let (_, _, code) = run_huck("((5+5))\n");
    assert_eq!(code, 0, "((5+5)) should be arith and exit 0");
}

#[test]
fn space_between_parens_is_still_subshell() {
    // `( :; )` with whitespace between `(`s continues to parse as
    // nested subshell. The `:` is the null command, exit 0.
    let (_, _, code) = run_huck("( :; )\n");
    assert_eq!(code, 0, "subshell with `:` should exit 0");
}
