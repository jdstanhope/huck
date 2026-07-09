//! v116: [^...] bracket negation in glob patterns (M-113).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
// Run via a temp script file, NOT piped stdin. huck's piped-stdin path goes
// through `read_logical_command` which performs `!`-history expansion (a
// documented, intentional behavior — see bash-divergences B-10 / the REPL/piped
// stdin reader note), so a fragment containing `[!0-9]` would trip `!0: event
// not found`. bash likewise only history-expands interactively; a non-interactive
// script (file arg, the comparison contract here) does not. Using a file arg is
// thus the apples-to-apples non-interactive execution path.
fn run(script: &str) -> (String, String, i32) {
    let mut path = std::env::temp_dir();
    path.push(format!("huck_bnt_{}_{}.sh", std::process::id(), {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }));
    std::fs::File::create(&path)
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = Command::new(huck_bin())
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
fn subst_negated_class_removes_complement() {
    assert_eq!(run("v=abc123\necho \"${v//[^0-9]/}\"\n").0, "123\n");
}
#[test]
fn remove_prefix_negated_class() {
    assert_eq!(run("v=abc123\necho \"${v#[^0-9]}\"\n").0, "bc123\n");
}
#[test]
#[ignore = "M-54: the glob crate does not support POSIX bracket classes \
            ([:digit:]), independent of [^...] negation; the ^->! translation \
            is exercised by subst_negated_class_removes_complement etc. and \
            verified for the POSIX case in the glob_match unit test \
            posix_class_inner_brackets. Bash output here is `12`."]
fn subst_negated_posix_class() {
    assert_eq!(run("v=ab12cd\necho \"${v//[^[:digit:]]/}\"\n").0, "12\n");
}
#[test]
fn case_negated_class() {
    assert_eq!(
        run("case A in [^0-9]) echo letter;; *) echo other;; esac\n").0,
        "letter\n"
    );
}
#[test]
fn dbracket_negated_class() {
    assert_eq!(run("[[ A == [^0-9] ]] && echo Y || echo N\n").0, "Y\n");
}
#[test]
fn bang_negation_still_works() {
    assert_eq!(run("v=abc123\necho \"${v//[!0-9]/}\"\n").0, "123\n");
}
#[test]
fn caret_not_leading_is_literal() {
    // `[a^b]` removes a, ^, b (^ literal in the class) -> "c"
    assert_eq!(run("v=a^bc\necho \"${v//[a^b]/}\"\n").0, "c\n");
}
#[test]
fn pathname_negated_class() {
    // Create files in a temp dir and glob with [^a].
    let (out, _e, _c) = run(
        "d=$(mktemp -d)\ntouch \"$d/afile\" \"$d/bfile\" \"$d/cfile\"\n\
         cd \"$d\"\nfor f in [^a]file; do echo \"$f\"; done\nrm -rf \"$d\"\n",
    );
    assert_eq!(out, "bfile\ncfile\n", "out: {out}");
}
