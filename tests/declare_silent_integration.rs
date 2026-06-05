//! v93: declare -f/-F on a missing function is silent (rc 1, no output).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Returns (stdout, stderr, exit_code).
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn declare_f_missing_silent() {
    let (so, se, _rc) = run("declare -f no_such_fn; echo rc=$?\n");
    assert_eq!(so, "rc=1\n");
    assert_eq!(se, "");
}

#[test]
fn declare_cap_f_missing_silent() {
    let (so, se, _) = run("declare -F no_such_fn; echo rc=$?\n");
    assert_eq!(so, "rc=1\n");
    assert_eq!(se, "");
}

#[test]
fn declare_cap_f_existing_prints() {
    // A defined function: -F exits 0 (rc check; stdout redirected to ignore format).
    let (so, _se, _) = run("f() { :; }\ndeclare -F f >/dev/null; echo rc=$?\n");
    assert_eq!(so, "rc=0\n");
}

#[test]
fn mise_style_probe_no_leak() {
    // The exact mise idiom: stderr NOT redirected, only stdout.
    let (_so, se, _) = run("declare -F _mise_hook >/dev/null; echo done\n");
    assert_eq!(se, "", "missing-function probe must not leak to stderr");
}
