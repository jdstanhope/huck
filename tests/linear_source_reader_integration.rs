//! v104: linear-time script source reader (M-99).
use std::io::Write;
use std::process::Command;

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Writes the script to a temp file and runs `huck <file>`.
/// Returns (stdout, stderr, exit_code).
fn run(script: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = dir.join(format!("huck_lsr_{pid}_{nanos}.sh"));
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(script.as_bytes()).expect("write temp script");
    }
    let out = Command::new(huck_bin())
        .arg(&path)
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
fn semicolon_list_runs_both() {
    let (out, _e, c) = run("echo a; echo b\n");
    assert_eq!(out, "a\nb\n");
    assert_eq!(c, 0);
}

#[test]
fn andor_list_short_circuits() {
    // `false && echo x` -> x skipped (lhs false); `true || echo y` -> y skipped
    // (lhs true). Bash produces no output here; huck must match byte-for-byte.
    let (out, _e, _c) = run("false && echo x; true || echo y\n");
    assert_eq!(out, "");
    // The short-circuit branches that DO fire still work:
    let (out2, _e, _c) = run("true && echo x; false || echo y\n");
    assert_eq!(out2, "x\ny\n");
}

#[test]
fn background_then_foreground() {
    let (out, _e, _c) = run("true & echo b\nwait\n");
    assert!(out.contains('b'));
}

#[test]
fn multiline_if_then_after() {
    let (out, _e, _c) = run("if true\nthen echo hi\nfi\necho after\n");
    assert_eq!(out, "hi\nafter\n");
}

#[test]
fn function_def_then_call() {
    let (out, _e, _c) = run("greet() {\n  echo hello\n}\ngreet\n");
    assert_eq!(out, "hello\n");
}

#[test]
fn heredoc_body_runs() {
    let (out, _e, _c) = run("cat <<EOF\nline1\nline2\nEOF\necho done\n");
    assert_eq!(out, "line1\nline2\ndone\n");
}

#[test]
fn set_v_echoes_subsequent_lines() {
    let (_o, err, _c) = run("set -v\necho one\nset +v\necho two\n");
    assert!(err.contains("echo one"));
    assert!(err.contains("set +v"));
    assert!(!err.contains("echo two"));
    assert!(!err.lines().any(|l| l == "set -v"));
}

#[test]
fn errexit_aborts_on_failure() {
    let (out, _e, c) = run("set -e\necho a\nfalse\necho b\n");
    assert_eq!(out, "a\n");
    assert_ne!(c, 0);
}

#[test]
fn exit_stops_script() {
    let (out, _e, c) = run("echo a\nexit 3\necho b\n");
    assert_eq!(out, "a\n");
    assert_eq!(c, 3);
}

#[test]
fn set_u_unbound_aborts_script() {
    let (out, _e, c) = run("set -u\necho a\necho \"$NOPE_UNDEF_XYZ\"\necho b\n");
    assert_eq!(out, "a\n");
    assert_ne!(c, 0);
}

#[test]
fn syntax_error_reports_line_and_continues() {
    let (out, err, _c) = run("echo a\n)\necho b\n");
    assert!(out.contains('a') && out.contains('b'));
    assert!(err.contains("line 2"), "stderr was: {err}");
}

#[test]
fn midfile_extglob_then_case_pattern() {
    let (out, _e, _c) = run(
        "shopt -s extglob\ncase abc in\n  @(abc|xyz)) echo hit ;;\n  *) echo miss ;;\nesac\n",
    );
    assert_eq!(out, "hit\n");
}

#[test]
fn large_single_logical_command_is_fast() {
    use std::time::Instant;
    let mut s = String::from("f() {\n");
    for i in 0..2000 {
        s.push_str(&format!("  x{i}=$(echo {i})\n"));
    }
    s.push_str("}\necho built\n");
    let t = Instant::now();
    let (out, _e, c) = run(&s);
    assert_eq!(out, "built\n");
    assert_eq!(c, 0);
    assert!(
        t.elapsed().as_secs() < 5,
        "took {:?}, expected < 5s (O(n^2) regression)",
        t.elapsed()
    );
}
