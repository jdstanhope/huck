//! v101: subshell / nested-arith inside command substitution $( … ).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn subshell_in_cmdsub() {
    assert_eq!(run("echo \"$( (echo a) )\"\n").0, "a\n");
}

#[test]
fn subshell_or_in_cmdsub() {
    assert_eq!(run("echo \"$( (echo a) || echo b )\"\n").0, "a\n");
}

#[test]
fn subshell_pipe_stage_in_cmdsub() {
    assert_eq!(run("echo \"$(echo a | (cat))\"\n").0, "a\n");
}

#[test]
fn subshell_with_semis_in_cmdsub() {
    assert_eq!(run("echo \"$( (exit 3); echo done )\"\n").0, "done\n");
}

#[test]
fn nested_arith_in_cmdsub() {
    assert_eq!(run("echo \"$( echo $((1 + 2)) )\"\n").0, "3\n");
}

#[test]
fn subshell_in_default_expansion() {
    assert_eq!(run("echo \"${x:-$( (echo d) )}\"\n").0, "d\n");
}

#[test]
fn subshell_in_array_literal() {
    assert_eq!(run("a=( \"$( (echo x) )\" )\necho \"${a[0]}\"\n").0, "x\n");
}

#[test]
fn nvm_resolve_alias_shape() {
    // $( (pipeline) || fallback ) — the exact nvm shape.
    assert_eq!(
        run("r=\"$( (printf 'a\\nb\\n' | head -n 1) || echo z )\"\necho \"$r\"\n").0,
        "a\n"
    );
}

#[test]
fn regression_plain_and_nested_cmdsub() {
    assert_eq!(
        run("echo \"$(echo a)\"\necho \"$(echo \"$(echo b)\")\"\n").0,
        "a\nb\n"
    );
}
