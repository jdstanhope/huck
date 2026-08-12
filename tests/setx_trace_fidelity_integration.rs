//! v130: set -x trace fidelity — simple-command quoting, command prefix,
//! decl args, inline-assignment lines.
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
fn trace_lines(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter(|l| l.starts_with("+ "))
        .map(String::from)
        .collect()
}

#[test]
fn quotes_arg_with_space() {
    let (_o, e, _c) = run("set -x\nx=\"a b\"; echo \"$x\" c\n");
    assert!(
        trace_lines(&e).contains(&"+ echo 'a b' c".to_string()),
        "stderr: {e}"
    );
}
#[test]
fn quotes_bracket_command() {
    let (_o, e, _c) = run("set -x\n[ 1 -lt 2 ]\n");
    assert!(
        trace_lines(&e).contains(&"+ '[' 1 -lt 2 ']'".to_string()),
        "stderr: {e}"
    );
}
#[test]
fn quotes_empty_and_special() {
    let (_o, e, _c) = run("set -x\necho \"\" \"; foo\"\n");
    assert!(
        trace_lines(&e).contains(&"+ echo '' '; foo'".to_string()),
        "stderr: {e}"
    );
}
#[test]
fn safe_words_stay_bare() {
    let (_o, e, _c) = run("set -x\necho hello a-b a/b a=b a,b\n");
    assert!(
        trace_lines(&e).contains(&"+ echo hello a-b a/b a=b a,b".to_string()),
        "stderr: {e}"
    );
}
#[test]
fn local_args_rendered() {
    let (_o, e, _c) = run("set -x\nf() { local DEF=x y; }; f\n");
    assert!(
        trace_lines(&e).contains(&"+ local DEF=x y".to_string()),
        "stderr: {e}"
    );
}
#[test]
fn command_prefix_kept() {
    let (_o, e, _c) = run("set -x\ncommand printf \"%s\\n\" hi\n");
    assert!(
        trace_lines(&e).contains(&"+ command printf '%s\\n' hi".to_string()),
        "stderr: {e}"
    );
}
#[test]
fn inline_assignment_separate_lines() {
    let (_o, e, _c) = run("set -x\nFOO=bar echo hi\n");
    let t = trace_lines(&e);
    let i = t.iter().position(|l| l == "+ FOO=bar").expect("FOO line");
    let j = t.iter().position(|l| l == "+ echo hi").expect("echo line");
    assert!(i < j, "FOO before echo; stderr: {e}");
}

#[test]
fn array_literal_decl_traces_as_two_lines() {
    // #372: bash traces a declaration builtin's compound value on its OWN
    // line, re-quoted, then the builtin with the operand reduced to the bare
    // NAME. This asserted the single `+ local arr=(a b c)` line huck used to
    // emit — the divergence itself. (The original point of the test, "must not
    // panic", is kept.)
    let (_o, e, c) = run("set -x\nf() { local arr=(a b c); }; f\n");
    assert_ne!(c, 101, "must not panic; stderr: {e}"); // 101 = Rust panic abort
    let t = trace_lines(&e);
    let i = t
        .iter()
        .position(|l| l == "+ arr=('a' 'b' 'c')")
        .unwrap_or_else(|| panic!("value line; stderr: {e}"));
    let j = t
        .iter()
        .position(|l| l == "+ local arr")
        .unwrap_or_else(|| panic!("builtin line; stderr: {e}"));
    assert!(i < j, "value line before the builtin line; stderr: {e}");
}

#[test]
fn bare_assignment_traced() {
    let (_o, e, _c) = run("set -x\nA=1\nB=\"x y\"\n");
    let t = trace_lines(&e);
    assert!(t.contains(&"+ A=1".to_string()), "stderr: {e}");
    assert!(t.contains(&"+ B='x y'".to_string()), "stderr: {e}");
}
#[test]
fn external_pipeline_stage_traced() {
    let (_o, e, _c) = run("set -x\necho a | cat\n");
    let t = trace_lines(&e);
    assert!(t.contains(&"+ echo a".to_string()), "stderr: {e}");
    assert!(t.contains(&"+ cat".to_string()), "stderr: {e}");
}
