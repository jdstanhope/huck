//! End-to-end tests for v30 [[ ]] extended test.

use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn dbracket_string_eq_true() {
    let (out, _) = run("[[ hello == hello ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_string_eq_false_sets_status() {
    let (out, _) = run("[[ hello == world ]]\necho $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn dbracket_pattern_match_glob() {
    let (out, _) = run("[[ hello.txt == *.txt ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_quoted_rhs_is_literal() {
    let (out, _) = run("[[ hello.txt == \"*.txt\" ]] || echo no\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "no"), "got: {out}");
}

#[test]
fn dbracket_regex_match() {
    let (out, _) = run("[[ hello42 =~ ^[a-z]+[0-9]+$ ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_regex_invalid_errors() {
    let (out, err) = run("[[ x =~ \"[\" ]]\necho $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "2"), "got out: {out} err: {err}");
}

#[test]
fn dbracket_int_eq() {
    let (out, _) = run("[[ 5 -eq 5 ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_int_gt() {
    let (out, _) = run("[[ 10 -gt 3 ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_int_arith_operand() {
    // In `[[ ]]` the integer-comparison ops arith-evaluate their operands:
    // a bare name resolves to its value (an unset name -> 0), so `abc -eq 5`
    // is `0 -eq 5` -> false -> rc 1 (matches bash). It is NOT a "bad integer".
    let (out, _) = run("[[ abc -eq 5 ]]\necho $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn dbracket_int_bare_name_resolves() {
    // A bare variable name on either side resolves to its numeric value.
    let (out, _) = run("x=10\n[[ x -eq 10 ]] && echo yes\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "yes"), "got: {out}");
}

#[test]
fn dbracket_file_test_existing() {
    let (out, _) = run("[[ -f /etc/hosts ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_file_test_missing() {
    let (out, _) = run("[[ ! -f /definitely/not/here ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_string_empty_z() {
    let (out, _) = run("[[ -z \"\" ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_string_nonempty_n() {
    let (out, _) = run("[[ -n hello ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_and_short_circuit_avoids_error() {
    // Second test would error if reached, but && short-circuits on false.
    let (out, _) = run("[[ -f /no/such && -r /no/such ]]\necho $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn dbracket_or_short_circuit() {
    let (out, _) = run("[[ hello == hello || -f /no/such ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_grouped_precedence() {
    let (out, _) = run("[[ ( a == a || b == c ) && d == d ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_no_word_splitting() {
    let (out, _) = run("FOO=\"a b\"\n[[ $FOO == \"a b\" ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_in_if() {
    let (out, _) = run("if [[ -f /etc/hosts ]]; then echo ok; fi\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_chained_with_and() {
    let (out, _) = run("[[ a == a ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_with_inline_assignment() {
    let (out, _) = run("FOO=hi [[ $FOO == hi ]] && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_in_subshell() {
    let (out, _) = run("([[ a == a ]]) && echo ok\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "ok"), "got: {out}");
}

#[test]
fn dbracket_in_while() {
    // Test that `[[ ]]` works as a while-loop condition. Loop runs once
    // when the counter is 0, increments, then [[ ]] becomes false and exits.
    let (out, _) = run("i=0\nwhile [[ $i -lt 1 ]]; do echo iter:$i; i=1; done\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "iter:0"), "got: {out}");
}
