//! Integration tests for v91 extglob pathname globbing (M-84a).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Builds a fresh temp fixture dir, runs `script` in it (cwd = fixture),
/// returns (stdout, exit_code).
fn run_in_fixture(script: &str) -> (String, i32) {
    let dir = std::env::temp_dir().join(format!(
        "huck_egpath_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["a", "b", "ab", "aab", "abc", "cd", "xy", ".hidden"] {
        std::fs::write(dir.join(f), b"").unwrap();
    }
    std::fs::create_dir(dir.join("dir1")).unwrap();
    std::fs::write(dir.join("dir1/foo.txt"), b"").unwrap();
    std::fs::write(dir.join("dir1/bar.log"), b"").unwrap();
    std::fs::create_dir(dir.join("dir2")).unwrap();
    std::fs::write(dir.join("dir2/foo.txt"), b"").unwrap();
    let mut child = Command::new(huck_bin())
        .current_dir(&dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn echo_extglob_expands_sorted() {
    assert_eq!(run_in_fixture("shopt -s extglob\necho +(a|b)\n").0, "a aab ab b\n");
    assert_eq!(run_in_fixture("shopt -s extglob\necho @(a|cd)\n").0, "a cd\n");
    assert_eq!(run_in_fixture("shopt -s extglob\necho dir*/+(foo|bar).txt\n").0, "dir1/foo.txt dir2/foo.txt\n");
}

#[test]
fn for_loop_over_extglob() {
    assert_eq!(
        run_in_fixture("shopt -s extglob\nfor f in @(a|cd); do printf '%s|' \"$f\"; done\necho\n").0,
        "a|cd|\n"
    );
}

#[test]
fn extglob_off_is_literal() {
    // extglob off: the lexer does NOT form an extglob group, so `(` is a normal
    // operator and `echo +(a|b)` is a syntax error — byte-identical to bash 5.x
    // (`bash: syntax error near unexpected token '('`, rc 2, empty stdout). This
    // is the pre-v91 default behavior, unchanged: an off/non-extglob field never
    // reaches the walker.
    assert_eq!(run_in_fixture("echo +(a|b)\n").0, "");
}

#[test]
fn quoted_extglob_is_literal() {
    // A quoted group is literal, never filesystem-expanded.
    assert_eq!(run_in_fixture("shopt -s extglob\necho \"+(a|b)\"\n").0, "+(a|b)\n");
}

#[test]
fn nullglob_extglob_no_match_empty() {
    assert_eq!(run_in_fixture("shopt -s extglob nullglob\necho zzz+(q)\n").0, "\n");
}

#[test]
fn no_match_is_literal() {
    assert_eq!(run_in_fixture("shopt -s extglob\necho zzz+(q)\n").0, "zzz+(q)\n");
}
