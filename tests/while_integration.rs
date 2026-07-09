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
fn while_counting_loop() {
    let (out, _) = run("i=0; while test $i -lt 3; do echo $i; i=$((i+1)); done\nexit\n");
    let nums: Vec<&str> = out
        .lines()
        .filter(|l| *l == "0" || *l == "1" || *l == "2")
        .collect();
    assert_eq!(nums, vec!["0", "1", "2"], "stdout: {out}");
}

#[test]
fn until_loop() {
    let (out, _) = run("n=3; until test $n -eq 0; do echo n$n; n=$((n-1)); done\nexit\n");
    assert!(out.lines().any(|l| l == "n3"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "n1"), "stdout: {out}");
    assert!(
        !out.lines().any(|l| l == "n0"),
        "n0 should not appear: {out}"
    );
}

#[test]
fn break_exits_loop_early() {
    let (out, _) = run(
        "i=0; while test $i -lt 100; do echo at-$i; i=$((i+1)); if test $i -eq 2; then break; fi; done\nexit\n",
    );
    assert!(out.lines().any(|l| l == "at-0"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "at-1"), "stdout: {out}");
    assert!(
        !out.lines().any(|l| l == "at-2"),
        "loop should have broken: {out}"
    );
}

#[test]
fn continue_skips_iteration() {
    let (out, _) = run(
        "i=0; while test $i -lt 4; do i=$((i+1)); if test $i -eq 2; then continue; fi; echo v$i; done\nexit\n",
    );
    assert!(out.lines().any(|l| l == "v1"), "stdout: {out}");
    assert!(
        !out.lines().any(|l| l == "v2"),
        "v2 should be skipped: {out}"
    );
    assert!(out.lines().any(|l| l == "v3"), "stdout: {out}");
}

#[test]
fn while_true_with_break() {
    let (out, _) = run("while true; do echo once; break; done\nexit\n");
    let count = out.lines().filter(|l| *l == "once").count();
    assert_eq!(count, 1, "stdout: {out}");
}

#[test]
fn nested_while() {
    let (out, _) = run(
        "i=0; while test $i -lt 2; do j=0; while test $j -lt 2; do echo $i-$j; j=$((j+1)); done; i=$((i+1)); done\nexit\n",
    );
    for pair in ["0-0", "0-1", "1-0", "1-1"] {
        assert!(out.lines().any(|l| l == pair), "missing {pair}: {out}");
    }
}

#[test]
fn stray_break_continues_script() {
    let (out, err) = run("break\necho alive\nexit\n");
    assert!(out.lines().any(|l| l == "alive"), "stdout: {out}");
    assert!(
        err.contains("only meaningful"),
        "expected diagnostic: {err:?}"
    );
}

#[test]
fn while_loop_status_after() {
    let (out, _) =
        run("i=0; while test $i -lt 1; do i=$((i+1)); test 1 -eq 2; done\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
}

#[test]
fn unterminated_while_is_syntax_error() {
    let (_, err) = run("while test 1 -eq 1; do echo x\nexit\n");
    assert!(err.to_lowercase().contains("syntax error"), "stderr: {err}");
}
