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
fn hash_empty_table_listing() {
    let (out, _, _) = run_capture("hash\nexit\n");
    assert!(
        out.lines().any(|l| l == "hash: hash table empty"),
        "stdout: {out:?}"
    );
}

#[test]
fn hash_p_then_list() {
    let (out, _, _) = run_capture("hash -p /foo a\nhash -l\nexit\n");
    assert!(
        out.lines().any(|l| l == "builtin hash -p /foo a"),
        "stdout: {out:?}",
    );
}

#[test]
fn hash_path_lookup_succeeds_for_sh() {
    let (out, _, _) = run_capture("hash sh\nrc=$?\necho rc=$rc\nhash -t sh\nexit\n");
    assert!(out.lines().any(|l| l == "rc=0"), "stdout: {out:?}");
    assert!(
        out.lines().any(|l| l.ends_with("/sh")),
        "expected a /sh path; stdout: {out:?}",
    );
}

#[test]
fn hash_path_lookup_fails_for_missing() {
    let (out, err, _) = run_capture("hash __no_such_cmd_xyzzy__\nrc=$?\necho rc=$rc\nexit\n");
    assert!(err.contains("not found"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn hash_r_clears() {
    let (out, _, _) = run_capture("hash -p /foo a\nhash -r\nhash -t a\nrc=$?\necho rc=$rc\nexit\n");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn hash_t_multi_format() {
    let (out, _, _) = run_capture("hash -p /foo a\nhash -p /bar b\nhash -t a b\nexit\n");
    assert!(out.lines().any(|l| l == "a\t/foo"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "b\t/bar"), "stdout: {out:?}");
}

#[test]
fn hash_d_then_lookup_fails() {
    let (out, _, _) =
        run_capture("hash -p /foo a\nhash -d a\nhash -t a\nrc=$?\necho rc=$rc\nexit\n");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}
