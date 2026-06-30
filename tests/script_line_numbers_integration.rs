//! v94: sourced-script syntax errors report the physical line number.
use std::io::Write;
use std::process::Command;

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Writes `body` to a temp file, runs `huck <file>`, returns (stdout, stderr, code).
fn run_script(body: &str) -> (String, String, i32) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script.sh");
    std::fs::File::create(&path).unwrap().write_all(body.as_bytes()).unwrap();
    let out = Command::new(huck_bin()).arg(&path).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn parse_error_reports_line() {
    // `fi` with no `if` is a parse error on line 3.
    let (_o, se, _c) = run_script("echo one\necho two\nfi\n");
    assert!(se.contains("line 3:"), "stderr missing 'line 3:': {se:?}");
    assert!(se.contains("syntax error"), "stderr missing 'syntax error': {se:?}");
}

#[test]
fn error_line_is_command_start_not_line_one() {
    // Valid commands first; the error is on line 4.
    let (_o, se, _c) = run_script("x=1\necho hi\n: ok\n)\n");
    assert!(se.contains("line 4:"), "expected 'line 4:', got: {se:?}");
}

#[test]
fn lex_error_reports_line() {
    // `${}` (parameter expansion with an empty name) parses fine and defers to
    // a RUNTIME "bad substitution" on line 2 (v233: lexable-but-invalid `${…}`
    // matches bash instead of aborting the parse). The physical-line guarantee
    // this file exists to verify still holds: the error reports `line 2:`.
    let (_o, se, _c) = run_script("echo ok\necho ${}\n");
    assert!(se.contains("line 2:"), "expected 'line 2:', got: {se:?}");
    assert!(se.contains("bad substitution"), "stderr: {se:?}");
}

#[test]
fn multiline_construct_points_at_first_line() {
    // A 3-line function whose body has a stray `done`; documented limitation:
    // the reported line is the construct's FIRST line (function def, line 2).
    let (_o, se, _c) = run_script("echo a\nf() {\n  done\n}\n");
    assert!(se.contains("syntax error"), "stderr: {se:?}");
    assert!(se.contains("line 2:"), "expected first-line 'line 2:', got: {se:?}");
}

// --- v239 regression guards: a lex error that begins a unit in the live source
// loop must be REPORTED (not silently swallowed) and at the failing token's
// physical line, not the cursor's post-scan EOF line. ---

#[test]
fn lex_error_as_only_unit_is_reported_line_one() {
    // Whole script is a single unterminated-quote token (no prior unit): the
    // newline-skip peek hits the lex error first. Must report, not stay silent.
    let (_o, se, c) = run_script("'unterminated\n");
    assert!(se.contains("syntax error"), "lex error must be reported: {se:?}");
    assert!(se.contains("line 1:"), "expected 'line 1:', got: {se:?}");
    assert_eq!(c, 2, "exit code should be 2, got {c}");
}

#[test]
fn lex_error_as_first_token_of_second_unit_reports_its_line() {
    // A clean unit, then a unit beginning with an unterminated quote. The error
    // surfaces via the post-unit boundary peek; it must report line 2 (the token's
    // start), not line 3 (where the failed scan ran the cursor to EOF).
    let (so, se, c) = run_script("echo ok\n'unterminated\n");
    assert!(so.contains("ok"), "first unit should run: {so:?}");
    assert!(se.contains("syntax error"), "lex error must be reported: {se:?}");
    assert!(se.contains("line 2:"), "expected 'line 2:', got: {se:?}");
    assert_eq!(c, 2, "exit code should be 2, got {c}");
}

#[test]
fn lex_error_after_blank_line_counts_the_blank() {
    // The blank line must be counted in the physical line: error on line 3.
    let (_o, se, _c) = run_script("echo a\n\n'bad\n");
    assert!(se.contains("syntax error"), "stderr: {se:?}");
    assert!(se.contains("line 3:"), "expected 'line 3:', got: {se:?}");
}
