//! v230: ulimit builtin — env-independent round-trips + errors vs bash.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run `script` as a file arg (true non-interactive path). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("huck_ulimit_{}_{}.sh", std::process::id(), n));
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(script.as_bytes()).unwrap();
    }
    let out = Command::new(huck_bin())
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn nofile_roundtrip() {
    // lowering RLIMIT_NOFILE is always permitted; the soft value reads back.
    let (o, _, c) = run_file("ulimit -n 64\nulimit -n\n");
    assert_eq!(o, "64\n");
    assert_eq!(c, 0);
}

#[test]
fn core_soft_roundtrip_within_hard() {
    // raise hard core first so the soft set is permitted regardless of env.
    let (o, _, _) = run_file("ulimit -c unlimited\nulimit -c -S -- 1000\nulimit -c\n");
    assert_eq!(o, "1000\n");
}

#[test]
fn unlimited_query() {
    let (o, _, _) = run_file("ulimit -c unlimited\nulimit -c\n");
    assert_eq!(o, "unlimited\n");
}

#[test]
fn invalid_number() {
    let (_, e, _) = run_file("ulimit -n abc\n");
    assert!(e.contains("ulimit: abc: invalid number"), "stderr: {e}");
}

#[test]
fn invalid_option() {
    let (_, e, _) = run_file("ulimit -Z\n");
    assert!(e.contains("ulimit: -Z: invalid option"), "stderr: {e}");
}
