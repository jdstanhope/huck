use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn type_default_for_builtin() {
    let (out, _, _) = run_capture("type echo\nexit\n");
    assert!(
        out.lines().any(|l| l == "echo is a shell builtin"),
        "stdout: {out:?}",
    );
}

#[test]
fn type_t_for_keyword() {
    let (out, _, _) = run_capture("type -t if\nexit\n");
    assert!(out.lines().any(|l| l == "keyword"), "stdout: {out:?}");
}

#[test]
fn type_p_for_builtin_is_empty() {
    let (out, _, _) = run_capture("type -p echo\nrc=$?\necho \"rc=$rc[$(type -p echo)]\"\nexit\n");
    // Builtin without a file: -p prints nothing on its own line,
    // and the rc + bracket sandwich confirms empty interior.
    assert!(out.contains("rc=0[]"), "stdout: {out:?}");
}

#[test]
fn type_p_for_file_returns_path() {
    let (out, _, _) = run_capture("type -p sh\nexit\n");
    assert!(
        out.lines().any(|l| l.ends_with("/sh")),
        "expected a path ending /sh; stdout: {out:?}",
    );
}

#[test]
fn type_not_found_exit_1() {
    let (out, err, _) = run_capture("type __no_such_command_xyzzy__\nrc=$?\necho rc=$rc\nexit\n");
    assert!(err.contains("not found"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn type_a_alias_then_path() {
    let (out, _, _) = run_capture("alias ls=foo\ntype -a ls\nexit\n");
    assert!(
        out.lines().any(|l| l.contains("aliased to `foo'")),
        "stdout: {out:?}",
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("ls is /") || l == "ls is /usr/bin/ls"),
        "expected at least one path line; stdout: {out:?}",
    );
}

#[test]
fn type_capital_p_force_path_for_sh() {
    let (out, _, _) = run_capture("type -P sh\nexit\n");
    assert!(
        out.lines().any(|l| l.ends_with("/sh")),
        "expected /sh path; stdout: {out:?}",
    );
}

#[test]
fn type_f_skips_function() {
    let (out, err, _) = run_capture("f() { :; }\ntype -f f\nrc=$?\necho rc=$rc\nexit\n");
    assert!(err.contains("not found"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}
