//! v230: times special builtin — two lines, shell then children, %dm%.3fs.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `script` as a file arg (true non-interactive path). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("huck_times_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn times_prints_two_lines_in_format() {
    let (o, _, c) = run_file("times\n");
    assert_eq!(c, 0);
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines.len(), 2, "times prints exactly two lines, got: {o:?}");
    // each line: "<m>m<s>.<ms>s <m>m<s>.<ms>s"
    for l in &lines {
        let cols: Vec<&str> = l.split(' ').collect();
        assert_eq!(cols.len(), 2, "two columns per line: {l:?}");
        for c in cols { assert!(c.contains('m') && c.ends_with('s'), "format {c:?}"); }
    }
}
