//! v125 (M-117): a redirect on a function-call command applies to the body.
//! File-arg execution (L-27).

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

fn run_huck_frag(frag: &str) -> (String, String, i32) {
    let path = std::env::temp_dir().join(format!(
        "huck_v125_{}_{}.sh",
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
fn func_redirect_to_file_writes_body_output() {
    let (out, _e, _c) =
        run_huck_frag(r#"f(){ printf '%s\n' BODY; }; d=$(mktemp -d); f >"$d/x"; cat "$d/x""#);
    assert_eq!(out.trim_end(), "BODY", "{out:?}");
}

#[test]
fn func_redirect_to_stderr_not_captured() {
    let (out, _e, _c) =
        run_huck_frag(r#"f(){ printf '%s\n' BODY; }; a=$(f >&2 2>/dev/null); echo "[$a]""#);
    assert_eq!(out.trim_end(), "[]", "{out:?}");
}

// NOTE: func_2to1_captures_stderr (b=$(g 2>&1)) is an L-25 residual — huck's
// execute_capturing uses an in-process Capture sink (Rust Vec), not a real fd 1
// pipe, so dup2(1,2) can't redirect into the capture buf. This test is omitted
// here; the gap is tracked as L-25. Bash forks for $(), making 2>&1 work there.

#[test]
fn func_stderr_suppressed() {
    let (_o, err, _c) = run_huck_frag(r#"g(){ printf '%s\n' OOPS >&2; }; g 2>/dev/null"#);
    assert!(
        !err.contains("OOPS"),
        "stderr should be suppressed: {err:?}"
    );
}

#[test]
fn func_redirect_with_inline_assignment() {
    let (out, _e, _c) =
        run_huck_frag(r#"f(){ printf '%s\n' "v=$V"; }; d=$(mktemp -d); V=1 f >"$d/x"; cat "$d/x""#);
    assert_eq!(out.trim_end(), "v=1", "{out:?}");
}

#[test]
fn func_body_builtin_and_external_both_redirected() {
    let (out, _e, _c) = run_huck_frag(
        r#"f(){ echo BUILTIN; command echo EXTERNAL; }; d=$(mktemp -d); f >"$d/x"; cat "$d/x""#,
    );
    assert!(
        out.contains("BUILTIN") && out.contains("EXTERNAL"),
        "{out:?}"
    );
}
