use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
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
fn read_single_name_via_heredoc() {
    let (out, _) = run_capture(
        "read X <<< 'hello'\necho \"[$X]\"\nexit\n",
    );
    assert!(out.lines().any(|l| l == "[hello]"), "stdout: {out:?}");
}

#[test]
fn read_multi_name_split() {
    let (out, _) = run_capture(
        "read X Y <<< 'a b c'\necho \"[$X][$Y]\"\nexit\n",
    );
    assert!(out.lines().any(|l| l == "[a][b c]"), "stdout: {out:?}");
}

#[test]
fn read_with_reply_default() {
    let (out, _) = run_capture(
        "read <<< 'hi there'\necho \"[$REPLY]\"\nexit\n",
    );
    assert!(out.lines().any(|l| l == "[hi there]"), "stdout: {out:?}");
}

#[test]
fn read_eof_returns_1() {
    let (out, _) = run_capture(
        "read X </dev/null\necho rc=$?\nexit\n",
    );
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn read_dash_r_preserves_backslash() {
    let (out, _) = run_capture(
        "read -r X <<< 'a\\b'\necho \"[$X]\"\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == r"[a\b]"),
        "stdout: {out:?}",
    );
}

#[test]
fn read_dash_d_custom_delim() {
    let (out, _) = run_capture(
        "read -d ':' X <<< 'foo:bar'\necho \"[$X]\"\nexit\n",
    );
    assert!(out.lines().any(|l| l == "[foo]"), "stdout: {out:?}");
}

#[test]
fn read_readonly_var_errors() {
    let (out, err) = run_capture(
        "readonly X=v\nread X <<< 'new'\nrc=$?\necho \"rc=$rc\"\necho \"[$X]\"\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "[v]"), "stdout: {out:?}");
}

#[test]
fn read_invalid_identifier_errors() {
    let (out, err) = run_capture(
        "read 1foo <<< 'x'\necho rc=$?\nexit\n",
    );
    assert!(err.contains("not a valid identifier"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

// Regression for the -s spurious-newline bug: `read -s` on a
// non-tty (heredoc here) must not emit a blank line to stderr.
#[test]
fn read_dash_s_silent_on_non_tty_emits_no_newline() {
    let (out, err) = run_capture(
        "read -s X <<< 'hi'\necho \"[$X]\"\nexit\n",
    );
    assert!(out.lines().any(|l| l == "[hi]"), "stdout: {out:?}");
    assert!(
        err.is_empty(),
        "stderr should be empty when -s reads from a non-tty; got: {err:?}",
    );
}
