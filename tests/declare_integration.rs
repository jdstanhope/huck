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
fn declare_bare_assigns() {
    let (out, _, _) = run_capture("declare X=hi\necho \"[$X]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[hi]"), "stdout: {out:?}");
}

#[test]
fn declare_p_prints_decl() {
    let (out, _, _) = run_capture("X=hi\ndeclare -p X\nexit\n");
    assert!(
        out.lines().any(|l| l == "declare -- X=\"hi\""),
        "stdout: {out:?}",
    );
}

#[test]
fn declare_r_is_readonly() {
    let (out, err, _) = run_capture(
        "declare -r X=hi\nX=new\nrc=$?\necho \"rc=$rc\"\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn declare_x_is_exported() {
    let (out, _, _) = run_capture(
        "declare -x X=hi\ndeclare -p X\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == "declare -x X=\"hi\""),
        "stdout: {out:?}",
    );
}

#[test]
fn declare_plus_x_unexports() {
    let (out, _, _) = run_capture(
        "declare -x X=hi\ndeclare +x X\ndeclare -p X\nexit\n",
    );
    // After +x, attrs should be -- not -x.
    assert!(
        out.lines().any(|l| l == "declare -- X=\"hi\""),
        "stdout: {out:?}",
    );
}

#[test]
fn declare_inside_function_is_local() {
    let (out, _, _) = run_capture(
        "f() { declare X_LOCAL_DECL=in; }\nf\necho \"[$X_LOCAL_DECL]\"\nexit\n",
    );
    // X should be unset after function returns.
    assert!(
        out.lines().any(|l| l == "[]"),
        "stdout: {out:?}",
    );
}

#[test]
#[allow(non_snake_case)]
fn declare_F_lists_functions() {
    let (out, _, _) = run_capture(
        "f() { :; }\ndeclare -F\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == "declare -f f"),
        "stdout: {out:?}",
    );
}

#[test]
fn typeset_alias_works() {
    let (out, err, _) = run_capture(
        "typeset -r X=hi\nX=new\nrc=$?\necho \"rc=$rc\"\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}
