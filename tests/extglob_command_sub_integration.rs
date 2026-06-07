//! v106 (M-101): the source reader executes the clean prefix before a
//! lex-failing trailing unit, so an early `shopt -s extglob` takes effect
//! before a later extglob-in-command-sub is re-lexed.
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
    let path = dir.join(format!("huck_xgcs_{pid}_{nanos}.sh"));
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
fn shopt_extglob_then_later_command_sub_in_same_chunk() {
    // The `!(x)` inside `$()` can only be lexed once extglob is on; the whole
    // chunk is tokenized with the extglob value AT CHUNK START (off), so it
    // fails to tokenize at line 3. The reader must still run lines 1+2 first.
    let (out, _e, _c) = run("shopt -s extglob\necho MARKER\necho $(echo /nonexist/!(x))\n");
    assert!(out.contains("MARKER"), "stdout: {out}");
}

#[test]
fn shopt_extglob_then_function_with_extglob_sub() {
    let script = "shopt -s extglob\nf() { echo $(printf '%s\\n' /nonexist/!(x)); }\nf\necho done\n";
    let (out, _e, _c) = run(script);
    assert!(out.contains("done"), "stdout: {out}");
}

#[test]
fn malformed_line_reports_once_no_loop() {
    // genuinely un-lexable -> report once and continue, NO hang.
    let (out, err, _c) = run("echo a\n$(\necho b\n");
    assert!(out.contains('a') || out.contains('b'));
    assert!(err.contains("syntax error"));
}

#[test]
fn clean_script_unaffected() {
    let (out, _e, c) = run("echo one\necho two\n");
    assert_eq!(out, "one\ntwo\n");
    assert_eq!(c, 0);
}
