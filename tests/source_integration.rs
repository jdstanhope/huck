use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn run_capture_with_path(script: &str, extra_path: &str) -> (String, String) {
    let path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{extra_path}:{path}");
    let mut child = Command::new(huck_binary())
        .env("PATH", &new_path)
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

/// Writes `content` to a unique tmp file and returns its path.
fn write_tmp(content: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v51_{pid}_{nanos}_{n}.sh"));
    std::fs::write(&path, content).expect("write tmp file");
    path
}

#[test]
fn source_runs_file_contents() {
    let tmp = write_tmp("echo HELLO\n");
    let script = format!("source {}\nexit\n", tmp.display());
    let (out, _) = run_capture(&script);
    assert!(
        out.lines().any(|l| l == "HELLO"),
        "expected HELLO in: {:?}",
        out
    );
}

#[test]
fn source_passes_extra_args_as_positional() {
    let tmp = write_tmp("echo \"$1 $2\"\n");
    let script = format!("source {} A B\nexit\n", tmp.display());
    let (out, _) = run_capture(&script);
    assert!(
        out.lines().any(|l| l == "A B"),
        "expected `A B` in: {:?}",
        out
    );
}

#[test]
fn source_return_early_exits() {
    let tmp = write_tmp("echo BEFORE\nreturn 0\necho SKIP\n");
    let script = format!("source {}\necho AFTER\nexit\n", tmp.display());
    let (out, _) = run_capture(&script);
    assert!(
        out.lines().any(|l| l == "BEFORE"),
        "expected BEFORE in: {:?}",
        out
    );
    assert!(
        !out.lines().any(|l| l == "SKIP"),
        "expected SKIP to be suppressed: {:?}",
        out
    );
    assert!(
        out.lines().any(|l| l == "AFTER"),
        "expected AFTER (host shell continues): {:?}",
        out
    );
}

#[test]
fn source_via_dot_alias() {
    let tmp = write_tmp("echo HELLO_DOT\n");
    let script = format!(". {}\nexit\n", tmp.display());
    let (out, _) = run_capture(&script);
    assert!(
        out.lines().any(|l| l == "HELLO_DOT"),
        "expected HELLO_DOT in: {:?}",
        out
    );
}

#[test]
fn source_path_lookup() {
    let tmp = write_tmp("echo PATH_HIT\n");
    let dir = tmp.parent().unwrap().to_path_buf();
    let basename = tmp.file_name().unwrap().to_string_lossy().to_string();
    let script = format!("source {basename}\nexit\n");
    let (out, _) = run_capture_with_path(&script, dir.to_string_lossy().as_ref());
    assert!(
        out.lines().any(|l| l == "PATH_HIT"),
        "expected PATH_HIT in: {:?}",
        out
    );
}
