//! Integration tests for v84: ${...} operands parse as words (metachars literal).
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
fn alt_operand_with_parens_and_expansion() {
    assert_eq!(run("x=v\necho \"[${x:+($x)}]\"\n").0, "[(v)]\n");
    assert_eq!(run("unset y\necho \"[${y:+($y)}]\"\n").0, "[]\n");
}

#[test]
fn default_operand_with_metachars_literal() {
    assert_eq!(run("unset y\necho \"[${y:-(a|b;c)}]\"\n").0, "[(a|b;c)]\n");
}

#[test]
fn debian_ps1_line_parses() {
    // The exact construct from the stock Debian ~/.bashrc PS1.
    let (out, code) =
        run("debian_chroot=\nPS1=\"${debian_chroot:+($debian_chroot)}\\u@\\h\"\necho ok\n");
    assert_eq!(code, 0);
    assert!(out.contains("ok"), "stdout: {out:?}");
}

#[test]
fn default_operand_unquoted_splits() {
    // unquoted ${y:-a b c} field-splits into 3 args
    assert_eq!(
        run("unset y\nfor w in ${y:-a b c}; do printf '%s|' \"$w\"; done; echo\n").0,
        "a|b|c|\n"
    );
}

#[test]
fn default_operand_quoted_stays_one() {
    assert_eq!(
        run("unset y\nfor w in \"${y:-a b c}\"; do printf '%s|' \"$w\"; done; echo\n").0,
        "a b c|\n"
    );
}

#[test]
fn substitution_pattern_with_parens() {
    assert_eq!(run("v='a(b)c'\necho \"${v/(b)/X}\"\n").0, "aXc\n");
}

#[test]
fn substring_offset_parenthesized_arith() {
    assert_eq!(run("v=abcdef\necho \"${v:(1+1):2}\"\n").0, "cd\n");
}
