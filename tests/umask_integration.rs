//! v230: umask builtin — file-mode (non-interactive) behavior vs bash.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `script` as a file arg (true non-interactive path). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("huck_umask_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn octal_roundtrip() {
    let (o, _, c) = run_file("umask 022\numask\n");
    assert_eq!(o, "0022\n"); assert_eq!(c, 0);
}

#[test]
fn symbolic_print() {
    let (o, _, _) = run_file("umask 022\numask -S\n");
    assert_eq!(o, "u=rwx,g=rx,o=rx\n");
}

#[test]
fn posix_reusable() {
    let (o, _, _) = run_file("umask 022\numask -p\n");
    assert_eq!(o, "umask 0022\n");
}

#[test]
fn posix_symbolic_reusable() {
    let (o, _, _) = run_file("umask 002\numask -p -S\n");
    assert_eq!(o, "umask -S u=rwx,g=rwx,o=rx\n");
}

#[test]
fn set_via_symbolic() {
    // bash prints the symbolic mask when -S is given alongside a mode arg
    let (o, _, _) = run_file("umask -S u=rwx,g=rwx,o=rx\numask\n");
    assert_eq!(o, "u=rwx,g=rwx,o=rx\n0002\n");
}

#[test]
fn octal_out_of_range_keeps_mask() {
    // bad octal must not change the mask; stderr names the bad arg; rc 1.
    let (o, e, c) = run_file("umask 022\numask 09\numask\n");
    assert!(e.contains("umask: 09: octal number out of range"), "stderr: {e}");
    assert_eq!(o, "0022\n", "mask must be unchanged"); assert_eq!(c, 0);
}

#[test]
fn invalid_symbolic_character() {
    let (_, e, _) = run_file("umask g=u\n");
    assert!(e.contains("umask: `u': invalid symbolic mode character"), "stderr: {e}");
}

#[test]
fn invalid_symbolic_operator() {
    let (_, e, _) = run_file("umask u:rwx\n");
    assert!(e.contains("umask: `:': invalid symbolic mode operator"), "stderr: {e}");
}

#[test]
fn invalid_option() {
    let (_, e, _) = run_file("umask -i\n");
    assert!(e.contains("umask: -i: invalid option"), "stderr: {e}");
    assert!(e.contains("umask: usage: umask [-p] [-S] [mode]"), "stderr: {e}");
}
