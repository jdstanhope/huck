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
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn arith_hex_literal() {
    let (out, _) = run("echo $((0x10))\nexit\n");
    assert!(out.lines().any(|l| l == "16"), "stdout: {out}");
}

#[test]
fn arith_octal_literal() {
    let (out, _) = run("echo $((010))\nexit\n");
    assert!(out.lines().any(|l| l == "8"), "stdout: {out}");
}

#[test]
fn arith_base_n_binary() {
    let (out, _) = run("echo $((2#1010))\nexit\n");
    assert!(out.lines().any(|l| l == "10"), "stdout: {out}");
}

#[test]
fn arith_mixed_bases() {
    // 0x10 (16) + 010 (8) + 2#10 (2) = 26
    let (out, _) = run("echo $((0x10 + 010 + 2#10))\nexit\n");
    assert!(out.lines().any(|l| l == "26"), "stdout: {out}");
}

#[test]
fn arith_bitwise_and() {
    let (out, _) = run("echo $((0xF0 & 0x0F))\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn arith_bitwise_or() {
    let (out, _) = run("echo $((0xF0 | 0x0F))\nexit\n");
    assert!(out.lines().any(|l| l == "255"), "stdout: {out}");
}

#[test]
fn arith_bitwise_xor() {
    let (out, _) = run("echo $((0xFF ^ 0x0F))\nexit\n");
    assert!(out.lines().any(|l| l == "240"), "stdout: {out}");
}

#[test]
fn arith_left_shift() {
    let (out, _) = run("echo $((1 << 8))\nexit\n");
    assert!(out.lines().any(|l| l == "256"), "stdout: {out}");
}

#[test]
fn arith_power() {
    let (out, _) = run("echo $((2 ** 10))\nexit\n");
    assert!(out.lines().any(|l| l == "1024"), "stdout: {out}");
}

#[test]
fn arith_assignment_persists_to_var() {
    // $((a = 5)) should print 5 AND set $a to 5.
    let (out, _) = run("echo $((a = 5))\necho $a\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.iter().any(|l| **l == *"5"), "stdout: {out}");
    // Both 5's appear.
    let count = lines.iter().filter(|l| ***l == *"5").count();
    assert_eq!(count, 2, "expected two '5' lines, got {count}; stdout: {out}");
}

#[test]
fn arith_compound_assignment() {
    let (out, _) = run("a=3\necho $((a += 4))\necho $a\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let sevens = lines.iter().filter(|l| ***l == *"7").count();
    assert_eq!(sevens, 2, "expected two '7' lines; stdout: {out}");
}

#[test]
fn arith_post_increment_in_expression() {
    // a=5; $((a++ + 1)) = old(5) + 1 = 6; then $a = 6.
    let (out, _) = run("a=5\necho $((a++ + 1))\necho $a\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let sixes = lines.iter().filter(|l| ***l == *"6").count();
    assert_eq!(sixes, 2, "expected two '6' lines; stdout: {out}");
}

#[test]
fn arith_shift_assign() {
    // a=1; a <<= 3 → 1 * 2^3 = 8. Both echo lines print "8".
    let (out, _) = run("a=1\necho $((a <<= 3))\necho $a\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let eights = lines.iter().filter(|l| ***l == *"8").count();
    assert_eq!(eights, 2, "expected two '8' lines; stdout: {out}");
}

#[test]
fn arith_prefix_decrement() {
    // a=5; --a → 4 (pre-decrement returns new value). $a is 4 after.
    let (out, _) = run("a=5\necho $((--a))\necho $a\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let fours = lines.iter().filter(|l| ***l == *"4").count();
    assert_eq!(fours, 2, "expected two '4' lines; stdout: {out}");
}
