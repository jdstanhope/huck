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
fn printf_literal_only() {
    let (out, _, _) = run_capture("printf 'hello\\n'\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out:?}");
}

#[test]
fn printf_s_cycling() {
    let (out, _, _) = run_capture("printf '%s\\n' a b c\nexit\n");
    let collected: Vec<&str> = out.lines().take(3).collect();
    assert_eq!(collected, vec!["a", "b", "c"], "stdout: {out:?}");
}

#[test]
fn printf_d_width_zero_pad() {
    let (out, _, _) = run_capture("printf '%05d\\n' 42\nexit\n");
    assert!(out.lines().any(|l| l == "00042"), "stdout: {out:?}");
}

#[test]
fn printf_hex_alt_form() {
    let (out, _, _) = run_capture("printf '%#x\\n' 255\nexit\n");
    assert!(out.lines().any(|l| l == "0xff"), "stdout: {out:?}");
}

#[test]
fn printf_b_processes_escapes() {
    let (out, _, _) = run_capture("printf '%b\\n' 'a\\tb'\nexit\n");
    assert!(out.lines().any(|l| l == "a\tb"), "stdout: {out:?}");
}

#[test]
fn printf_b_c_halts_output() {
    // `printf '%b' 'a\cb'; echo X` → stdout begins "a" then "X".
    // No trailing newline from printf (no \n in format), no "b"
    // beyond the \c.
    let (out, _, _) = run_capture("printf '%b' 'a\\cb'\necho X\nexit\n");
    // The `a` and `X` should be on the same line (since printf
    // produced no newline). Echo's newline lands after "X".
    assert!(
        out.starts_with("aX\n") || out.starts_with("aX"),
        "expected stdout to start with `aX`, got: {out:?}",
    );
}

#[test]
fn printf_v_var_captures() {
    let (out, _, _) = run_capture("printf -v X '%d' 42\necho \"[$X]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[42]"), "stdout: {out:?}");
}

#[test]
fn printf_v_readonly_errors() {
    let (out, err, _) =
        run_capture("readonly X=v\nprintf -v X '%d' 42\nrc=$?\necho \"rc=$rc [$X]\"\nexit\n");
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1 [v]"), "stdout: {out:?}");
}

#[test]
fn printf_invalid_int_status_1() {
    let (out, err, _) = run_capture("printf '%d\\n' abc\nrc=$?\necho rc=$rc\nexit\n");
    assert!(err.contains("invalid number"), "stderr: {err:?}");
    // The parsed-prefix value of "abc" is 0; printf emits "0\n".
    assert!(out.lines().any(|l| l == "0"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn printf_no_args_usage_error() {
    // Capture printf's status via $? — huck's bare `exit` does not
    // inherit last_status (pre-existing divergence outside v56
    // scope), so we observe printf's exit code via $? directly.
    let (out, err, _) = run_capture("printf\nrc=$?\necho rc=$rc\nexit\n");
    assert!(err.contains("usage"), "stderr: {err:?}");
    assert!(
        out.lines().any(|l| l == "rc=2"),
        "expected printf to set status 2; stdout: {out:?}",
    );
}
