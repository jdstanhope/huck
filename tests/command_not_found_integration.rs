//! v228: command-not-found error format (word order + non-interactive prologue).
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run huck with a script FILE (not stdin) so the non-interactive prologue
/// (`<path>: line N:`) is produced. Returns (stdout, stderr, exit_code).
fn run_file(script: &str) -> (String, String, i32) {
    // Unique path per call: these #[test]s run in parallel threads sharing one
    // PID, so a PID-only name would race (one test's script overwriting another).
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck-cnf-{}-{n}.sh", std::process::id()));
    std::fs::write(&path, script).unwrap();
    let out = Command::new(huck_bin())
        .arg(&path)
        .output()
        .expect("run huck file");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn missing_command_uses_bash_word_order_and_prologue() {
    let (_o, e, c) = run_file("nosuch_cmd_xyz\n");
    assert!(
        e.contains(": line 1: nosuch_cmd_xyz: command not found"),
        "expected bash word order + prologue, got: {e:?}"
    );
    assert!(
        !e.contains("command not found: nosuch_cmd_xyz"),
        "old format still present: {e:?}"
    );
    assert!(
        !e.starts_with("huck:"),
        "file mode should not use the huck: prologue: {e:?}"
    );
    assert_eq!(c, 127);
}

#[test]
fn missing_command_reports_its_line_number() {
    // Missing command on line 3 → the prologue must say line 3.
    let (_o, e, c) = run_file("x=1\n: ok\nnosuch_cmd_xyz\n");
    assert!(
        e.contains(": line 3: nosuch_cmd_xyz: command not found"),
        "stderr: {e:?}"
    );
    assert_eq!(c, 127);
}

#[test]
fn quoted_empty_command_uses_bash_format() {
    // `''` is a real empty FIELD → site 5327 with an empty program name.
    // bash: `<path>: line 1: : command not found`.
    let (_o, e, c) = run_file("''\n");
    assert!(e.contains(": line 1: : command not found"), "stderr: {e:?}");
    assert!(
        !e.contains("command not found: "),
        "old format still present: {e:?}"
    );
    assert_eq!(c, 127);
}
