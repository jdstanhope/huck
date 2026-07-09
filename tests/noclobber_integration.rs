//! v123: noclobber (set -C) + >| force-clobber redirect (M-21).
//! Drives the `huck` binary via a temp file arg (file-arg execution, L-27).

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn run_huck_frag(frag: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v123_{}_{}.sh", std::process::id(), n));
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(frag.as_bytes()).expect("write temp script");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn huck");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn blocked_overwrite_keeps_file_and_errors() {
    let (out, err, _code) = run_huck_frag(
        r#"d=$(mktemp -d); echo orig > "$d/f"; set -C; echo new > "$d/f"; echo "c=$(cat "$d/f")""#,
    );
    assert!(out.contains("c=orig"), "file must be untouched: {out}");
    assert!(
        err.contains("cannot overwrite existing file"),
        "stderr: {err}"
    );
}

#[test]
fn force_clobber_overwrites() {
    let (out, _err, _code) = run_huck_frag(
        r#"d=$(mktemp -d); echo orig > "$d/f"; set -C; echo new >| "$d/f"; cat "$d/f""#,
    );
    assert!(out.contains("new"), "{out}");
}

#[test]
fn devnull_exempt_under_noclobber() {
    let (out, _e, code) = run_huck_frag(r#"set -C; echo x > /dev/null; echo done"#);
    assert_eq!(code, 0);
    assert!(
        out.contains("done"),
        "command after /dev/null redirect must run: {out}"
    );
}

#[test]
fn blocked_redirect_command_exit_status_is_1() {
    let (_o, _e, code) =
        run_huck_frag(r#"d=$(mktemp -d); echo orig > "$d/f"; set -C; echo new > "$d/f""#);
    assert_eq!(code, 1, "a redirect-blocked command exits 1");
}

#[test]
fn pipeline_stage_force_clobber() {
    let (out, _e, _c) = run_huck_frag(
        r#"d=$(mktemp -d); echo orig > "$d/f"; set -C; echo hi | cat >| "$d/f"; cat "$d/f""#,
    );
    assert!(
        out.contains("hi"),
        "pipeline-stage >| must force-overwrite: {out}"
    );
}
