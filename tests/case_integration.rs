use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with `script` piped to stdin; returns (stdout, stderr).
fn run(script: &str) -> (String, String) {
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
    )
}

#[test]
fn case_basic_match() {
    let (out, _) = run("case hello in hi) echo a;; hello) echo b;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "b"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "a"), "stdout: {out}");
}

#[test]
fn case_glob_pattern() {
    let (out, _) = run("case report.txt in *.txt) echo text;; *) echo other;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "text"), "stdout: {out}");
}

#[test]
fn case_question_mark_pattern() {
    let (out, _) = run("case ab in ??) echo two;; ?) echo one;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "two"), "stdout: {out}");
}

#[test]
fn case_alternation() {
    let (out, _) = run("case b in a|b|c) echo in-list;; *) echo no;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "in-list"), "stdout: {out}");
}

#[test]
fn case_catch_all_star() {
    let (out, _) = run("case zzz in a) echo a;; *) echo fallback;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "fallback"), "stdout: {out}");
}

#[test]
fn case_no_match_runs_nothing() {
    let (out, _) = run("case x in y) echo no;; esac\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "no"), "stdout: {out}");
}

#[test]
fn case_multiline() {
    let script = "case dog in\n  cat) echo meow ;;\n  dog) echo woof ;;\nesac\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "woof"), "stdout: {out}");
}

#[test]
fn case_subject_is_variable() {
    let (out, _) = run("x=apple\ncase $x in apple) echo fruit;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "fruit"), "stdout: {out}");
}

#[test]
fn case_fall_through() {
    let (out, _) = run("case a in a) echo one ;& b) echo two ;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "one"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "two"), "stdout: {out}");
}

#[test]
fn case_continue_match() {
    // both `a*` and `*b` match "ab"; ;;& keeps testing
    let (out, _) = run("case ab in a*) echo first ;;& *b) echo second ;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "first"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "second"), "stdout: {out}");
}

#[test]
fn case_leading_paren_form() {
    let (out, _) = run("case a in (a) echo paren;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "paren"), "stdout: {out}");
}

#[test]
fn case_quoted_metacharacter_is_literal() {
    // the pattern "*" (quoted) matches only the literal string *, not abc
    let (out1, _) = run("case abc in \"*\") echo wild;; *) echo other;; esac\nexit\n");
    assert!(out1.lines().any(|l| l == "other"), "quoted * must not match abc: {out1}");
    let (out2, _) = run("case * in \"*\") echo literal;; esac\nexit\n");
    assert!(out2.lines().any(|l| l == "literal"), "quoted * should match \"*\": {out2}");
}

#[test]
fn case_nested_in_for() {
    let (out, _) = run(
        "for x in a b c; do case $x in b) echo got-b;; *) echo skip-$x;; esac; done\nexit\n",
    );
    assert!(out.lines().any(|l| l == "got-b"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "skip-a"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "skip-c"), "stdout: {out}");
}

#[test]
fn break_from_case_inside_loop() {
    // `break` inside a case body targets the enclosing while loop
    let script = "i=0\nwhile test $i -lt 5; do i=$((i+1)); case $i in 3) break;; *) echo n$i;; esac; done\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "n1"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "n2"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "n3"), "loop should have broken: {out}");
    assert!(!out.lines().any(|l| l == "n4"), "loop should have broken: {out}");
}

#[test]
fn case_empty_body() {
    let (out, _) = run("case x in x) ;; *) echo other;; esac\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "other"), "stdout: {out}");
}
