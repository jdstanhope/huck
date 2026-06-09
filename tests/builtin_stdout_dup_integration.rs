//! v124 Fix B: builtins must honor a `>&N` stdout redirect.
//! A builtin's `>&2` must go to fd 2 (not stdout). File-arg execution (L-27).

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

fn run_huck_frag(frag: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "huck_v124_{}_{}.sh",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let mut f = std::fs::File::create(&path).expect("create temp script");
    f.write_all(frag.as_bytes()).expect("write temp script");
    drop(f);
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg(&path)
        .output()
        .expect("run huck");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn echo_to_stderr_not_captured_on_stdout() {
    let (out, _err, _c) = run_huck_frag(r#"a=$(echo Z >&2); echo "[$a]""#);
    assert_eq!(out.trim_end(), "[]", "stdout capture must be empty, got {out:?}");
}

#[test]
fn printf_to_stderr_not_captured() {
    let (out, _e, _c) = run_huck_frag(r#"a=$(printf '%s\n' Z >&2); echo "[$a]""#);
    assert_eq!(out.trim_end(), "[]", "{out:?}");
}

#[test]
fn echo_redirect_to_2_reaches_stderr() {
    let (_o, err, _c) = run_huck_frag(r#"echo HELLO >&2"#);
    assert!(err.contains("HELLO"), "stderr must contain HELLO, got {err:?}");
}

#[test]
fn echo_ampersand1_still_stdout() {
    let (out, _e, _c) = run_huck_frag(r#"echo KEEP >&1"#);
    assert!(out.contains("KEEP"), "{out:?}");
}

#[test]
fn func_err_to_stderr_suppressed_by_caller_redirect() {
    let (out, _e, _c) = run_huck_frag(
        r#"f() { >&2 printf '%s\n' MSG; }; a=$( (f 2>/dev/null) ); echo "[$a]""#,
    );
    assert_eq!(out.trim_end(), "[]", "{out:?}");
}
