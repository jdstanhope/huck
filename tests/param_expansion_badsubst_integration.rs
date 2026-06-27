//! v233: lexable-but-invalid ${...} defers to a runtime "bad substitution"
//! (matching bash) instead of a parse abort.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v233bs_{}_{}_.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn bad_subst_errors_at_runtime_not_parse() {
    let (_o, e, _c) = run_file("echo before\necho ${$x}\necho after\n");
    // Parses (so "before" runs); the bad subst errors at runtime.
    assert!(e.contains("bad substitution"), "stderr: {e}");
}

#[test]
fn bad_subst_short_circuited_does_not_error() {
    // ${H*} behind a short-circuit is never evaluated -> no error, rc 0.
    let (_o, e, c) = run_file("[[ -n yes || -z ${H*} ]]\necho rc=$?\n");
    assert!(!e.contains("bad substitution"), "should not error: {e}");
    assert_eq!(c, 0);
}

#[test]
fn bad_subst_message_reports_whole_word() {
    // bash reports the ENTIRE enclosing word's source in the error, not just
    // the offending `${…}` token: `echo a${-3}b` -> `a${-3}b: bad substitution`.
    let (_o, e, _c) = run_file("echo a${-3}b\n");
    assert!(e.contains("a${-3}b: bad substitution"), "stderr: {e}");
    assert!(!e.contains(" ${-3}: bad"), "should report whole word, got: {e}");
}

#[test]
fn bad_subst_message_whole_word_quoted() {
    // Quoted word: quotes are stripped but the `${…}` stays raw: `"[${-3}]"`
    // -> `[${-3}]: bad substitution`.
    let (_o, e, _c) = run_file("echo \"[${-3}]\"\n");
    assert!(e.contains("[${-3}]: bad substitution"), "stderr: {e}");
}

#[test]
fn bad_subst_whole_word_in_assignment_rhs() {
    // The whole-word message also applies off the command-argument path:
    // an assignment RHS reports the full word `a${-3}b`, not just the token.
    let (_o, e, _c) = run_file("x=a${-3}b\n");
    assert!(e.contains("a${-3}b: bad substitution"), "stderr: {e}");
    assert!(!e.contains(" ${-3}: bad"), "should report whole word, got: {e}");
}
