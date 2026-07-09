//! v110: genuinely zero-error mise activate.
//! Part A (M-90 combined `>file 2>&1`) + Part B (M-105 spurious empty field).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Returns (stdout, stderr, exit_code).
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

// --- Part A: M-90 combined `>file 2>&1` ---

#[test]
fn combined_redirect_suppresses_builtin_stderr() {
    // `declare -p NOPE >/dev/null 2>&1` (mise line 29 shape): the builtin's
    // error must go to /dev/null, not leak to the real stderr.
    let (out, err, _c) = run("declare -p NOPEA >/dev/null 2>&1\necho ok\n");
    assert_eq!(out, "ok\n", "out: {out}");
    assert!(!err.contains("NOPEA"), "builtin stderr leaked: {err}");
}

#[test]
fn file_redirect_still_suppresses() {
    // v109 file path must still work.
    let (out, err, _c) = run("declare -p NOPEB 2>/dev/null\necho ok\n");
    assert_eq!(out, "ok\n", "out: {out}");
    assert!(!err.contains("NOPEB"), "stderr leaked: {err}");
}

#[test]
fn bare_2to1_still_pipes() {
    // bare `2>&1` (no stdout file) must still route builtin stderr into the pipe.
    let (out, _err, _c) = run("{ declare -p NOPEC 2>&1; } | grep -c NOPEC\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn unredirected_builtin_error_still_reaches_stderr() {
    // No stderr redirect → error still hits the real fd 2 (must-not-regress).
    let (_o, err, _c) = run("declare -p NOPED\n");
    assert!(
        err.contains("NOPED"),
        "unredirected error should reach stderr: {err}"
    );
}

// --- Part B: M-105 unquoted `${x+alt}` spurious empty field ---

#[test]
fn unquoted_empty_alt_no_spurious_field() {
    // `${u+X}` unset, unquoted, followed by more words: must NOT inject an
    // empty leading field. bash: $# == 2.
    let (out, _e, _c) = run("set -- ${u+X} a b\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn empty_array_alt_no_spurious_field() {
    // The mise shape: empty array + `${arr[@]+"${arr[@]}"}` -> nothing.
    let (out, _e, _c) = run("f=()\nset -- ${f[@]+\"${f[@]}\"} -s bash\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn quoted_empty_alt_still_one_field() {
    // A QUOTED empty must still be one field. bash: $# == 2.
    let (out, _e, _c) = run("set -- \"${u+x}\" a\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn quoted_empty_field_printf() {
    // `printf '<%s>' "${u+x}"` (unset, whole-quoted) -> one empty field `<>`.
    let (out, _e, _c) = run("printf '<%s>' \"${u+x}\"\necho\n");
    assert_eq!(out, "<>\n", "out: {out}");
}

#[test]
fn set_array_idiom_unchanged() {
    // v109 behavior must be unchanged: a SET array still yields its elements.
    let (out, _e, _c) = run("a=(1 2)\nprintf '<%s>' \"${a[@]+\"${a[@]}\"}\"\necho\n");
    assert_eq!(out, "<1><2>\n", "out: {out}");
}
