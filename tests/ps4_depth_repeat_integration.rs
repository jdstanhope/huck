//! v131: PS4 depth-repeat (first char of PS4 repeated by command-sub/eval
//! nesting) + PS4 expansion (escapes + $VAR via expand_prompt).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
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
fn lines(stderr: &str) -> Vec<String> {
    stderr.lines().map(String::from).collect()
}
fn has(stderr: &str, line: &str) -> bool {
    lines(stderr).iter().any(|l| l == line)
}

#[test]
fn nested_command_sub_depth() {
    let (_o, e, _c) = run("set -x\na=$(echo $(echo hi))\n");
    assert!(has(&e, "+++ echo hi"), "stderr: {e}");
    assert!(has(&e, "++ echo hi"), "stderr: {e}");
    assert!(has(&e, "+ a=hi"), "stderr: {e}");
}

#[test]
fn command_sub_in_function_depth() {
    let (_o, e, _c) = run("set -x\nf() { echo $(echo x); }\nf\n");
    assert!(has(&e, "+ f"), "stderr: {e}");
    assert!(has(&e, "++ echo x"), "stderr: {e}");
    assert!(has(&e, "+ echo x"), "stderr: {e}");
}

#[test]
fn eval_adds_depth() {
    let (_o, e, _c) = run("set -x\neval \"echo ev\"\n");
    assert!(has(&e, "+ eval 'echo ev'"), "stderr: {e}");
    assert!(has(&e, "++ echo ev"), "stderr: {e}");
}

#[test]
fn function_call_no_depth() {
    let (_o, e, _c) = run("set -x\ng() { echo y; }\nf() { g; }\nf\n");
    assert!(has(&e, "+ f"), "stderr: {e}");
    assert!(has(&e, "+ g"), "stderr: {e}");
    assert!(has(&e, "+ echo y"), "stderr: {e}");
}

#[test]
fn subshell_no_depth() {
    let (_o, e, _c) = run("set -x\n( echo s )\n");
    assert!(has(&e, "+ echo s"), "stderr: {e}");
}

#[test]
fn custom_first_char_repeats() {
    let (_o, e, _c) = run("set -x\nPS4='> '\na=$(echo hi)\n");
    assert!(has(&e, ">> echo hi"), "stderr: {e}");
    assert!(has(&e, "> a=hi"), "stderr: {e}");
}

#[test]
fn multi_char_ps4_repeats_first_only() {
    let (_o, e, _c) = run("set -x\nPS4='XY '\na=$(echo hi)\n");
    assert!(has(&e, "XXY echo hi"), "stderr: {e}");
    assert!(has(&e, "XY a=hi"), "stderr: {e}");
}

#[test]
fn ps4_var_expansion() {
    let (_o, e, _c) = run("P=Q\nset -x\nPS4='$P '\necho z\n");
    assert!(has(&e, "Q echo z"), "stderr: {e}");
}

#[test]
fn default_ps4_no_regression() {
    let (_o, e, _c) = run("set -x\necho hi\n");
    assert!(has(&e, "+ echo hi"), "stderr: {e}");
}
