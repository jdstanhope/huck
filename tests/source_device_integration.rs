//! v231 A+B: source path resolution (CWD/sourcepath fallback) + device files.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

fn unique(tag: &str, ext: &str) -> std::path::PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "huck_v231_{tag}_{}_{}.{ext}",
        std::process::id(),
        n
    ))
}

/// Run `script` as a file arg (non-interactive). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let path = unique("s", "sh");
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

/// Run `script` (file arg) with `feed` piped to huck's stdin.
fn run_file_stdin(script: &str, feed: &str) -> (String, i32) {
    let path = unique("p", "sh");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(script.as_bytes()).unwrap();
    }
    let mut child = Command::new(huck_bin())
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(feed.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn source_cwd_fallback_sourcepath_off() {
    // shopt -u sourcepath; a bare filename present in CWD is sourced from CWD.
    let dir = unique("d", "dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("src4.sub"), "set -- m n o p\n").unwrap();
    let script = format!(
        "shopt -u sourcepath\ncd {}\n. src4.sub\necho \"$@\"\n",
        dir.display()
    );
    let (o, _, c) = run_file(&script);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(o, "m n o p\n");
    assert_eq!(c, 0);
}

#[test]
fn source_cwd_fallback_sourcepath_on() {
    // default sourcepath on: not in PATH → CWD fallback still sources it.
    let dir = unique("d2", "dir");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.sub"), "echo SOURCED\n").unwrap();
    let script = format!("cd {}\n. s.sub\n", dir.display());
    let (o, _, _) = run_file(&script);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(o, "SOURCED\n");
}

#[test]
fn source_dev_null() {
    let (o, _, c) = run_file(". /dev/null\necho \"rc=$?\"\n");
    assert_eq!(o, "rc=0\n");
    assert_eq!(c, 0);
}

#[test]
fn source_dev_stdin_runs_piped_content() {
    let (o, c) = run_file_stdin(". /dev/stdin\necho done\n", "echo PIPED-OK\n");
    assert_eq!(o, "PIPED-OK\ndone\n");
    assert_eq!(c, 0);
}

#[test]
fn source_missing_still_errors() {
    let (_, e, c) = run_file(". /no/such_xyz_v231\n");
    assert!(e.contains("No such file or directory"), "stderr: {e}");
    assert!(!e.contains("os error"), "leaks rust io text: {e}");
    assert_ne!(c, 0);
}

#[test]
fn source_directory_still_is_a_directory() {
    let (_, e, _) = run_file(". /etc\n");
    assert!(
        e.contains(".: /etc: is a directory") || e.contains("/etc: is a directory"),
        "stderr: {e}"
    );
}
