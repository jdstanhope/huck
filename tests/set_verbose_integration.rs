//! Integration tests for v89 `set -v` verbose mode (M-08e).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String { env!("CARGO_BIN_EXE_huck").to_string() }

/// Pipes `script` to huck on stdin (exercises read_logical_command).
/// Returns (stdout, stderr, exit_code).
fn run_capture(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (String::from_utf8_lossy(&out.stdout).to_string(),
     String::from_utf8_lossy(&out.stderr).to_string(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn verbose_echoes_to_stderr_not_stdout() {
    let (out, err, _) = run_capture("set -v\necho hi\n");
    assert_eq!(out, "hi\n");
    assert_eq!(err, "echo hi\n"); // `set -v` line itself is NOT echoed
}

#[test]
fn verbose_enable_disable_ordering() {
    // read->echo->execute: `set -v` not echoed; `echo b` + `set +v` echoed; `echo c` not.
    let (out, err, _) = run_capture("echo a\nset -v\necho b\nset +v\necho c\n");
    assert_eq!(out, "a\nb\nc\n");
    assert_eq!(err, "echo b\nset +v\n");
}

#[test]
fn verbose_echoes_each_continuation_line() {
    let (_, err, _) = run_capture("set -v\nif true\nthen echo x\nfi\n");
    assert!(err.contains("if true\n"), "stderr: {err:?}");
    assert!(err.contains("then echo x\n"), "stderr: {err:?}");
    assert!(err.contains("fi\n"), "stderr: {err:?}");
}

#[test]
fn verbose_dollar_dash_has_v() {
    let (out, _, _) = run_capture("set -v\necho $-\n");
    assert!(out.contains('v'), "stdout: {out:?}");
}

#[test]
fn verbose_echoes_sourced_file_lines() {
    // Exercises run_sourced_contents: `source FILE` line echoed by the reader,
    // and FILE's own lines echoed by run_sourced_contents.
    let dir = std::env::temp_dir().join(format!("huck_verbose_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let f = dir.join("sourced.sh");
    std::fs::write(&f, "echo sourced\n").unwrap();
    let script = format!("set -v\nsource {}\n", f.display());
    let (out, err, _) = run_capture(&script);
    assert_eq!(out, "sourced\n");
    assert!(err.contains(&format!("source {}", f.display())), "stderr: {err:?}");
    assert!(err.contains("echo sourced\n"), "stderr: {err:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
