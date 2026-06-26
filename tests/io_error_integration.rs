//! v229: io::Error text (no `(os error N)` suffix) + prologue on file-IO sites.
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run huck with a script FILE so the non-interactive prologue is produced.
fn run_file(script: &str) -> (String, String, i32) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck-ioe-{}-{n}.sh", std::process::id()));
    std::fs::write(&path, script).unwrap();
    let out = Command::new(huck_bin()).arg(&path).output().expect("run huck file");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn cd_missing_has_no_os_error_suffix_and_prologue() {
    let (_o, e, _c) = run_file("cd /no/such_xyz\n");
    assert!(e.contains(": line 1: cd: /no/such_xyz: No such file or directory\n"), "stderr: {e:?}");
    assert!(!e.contains("os error"), "leaked Rust suffix: {e:?}");
    assert!(!e.starts_with("huck:"), "file mode should not use huck: prologue: {e:?}");
}

#[test]
fn cd_into_file_reports_not_a_directory() {
    let (_o, e, _c) = run_file("cd /etc/hostname\n");
    assert!(e.contains(": line 1: cd: /etc/hostname: Not a directory\n"), "stderr: {e:?}");
    assert!(!e.contains("os error"), "leaked Rust suffix: {e:?}");
}

#[test]
fn redirect_read_missing_has_prologue_no_suffix() {
    let (_o, e, _c) = run_file("cat < /no/such_xyz\n");
    assert!(e.contains(": line 1: /no/such_xyz: No such file or directory\n"), "stderr: {e:?}");
    assert!(!e.contains("os error"), "leaked Rust suffix: {e:?}");
}

#[test]
fn redirect_write_to_directory_is_a_directory() {
    let (_o, e, _c) = run_file("echo hi > /etc\n");
    assert!(e.contains(": line 1: /etc: Is a directory\n"), "stderr: {e:?}");
    assert!(!e.contains("os error"), "leaked Rust suffix: {e:?}");
}
