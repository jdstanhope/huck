//! v231 C: shopt expand_aliases honored in non-interactive (file) mode.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run `script` as a file arg (non-interactive). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v231al_{}_{}_.sh", std::process::id(), n));
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(script.as_bytes()).unwrap();
    }
    let out = Command::new(huck_bin())
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn expand_aliases_def_then_use_across_lines() {
    let (o, _, c) = run_file("shopt -s expand_aliases\nalias foo='echo HELLO'\nfoo\n");
    assert_eq!(o, "HELLO\n");
    assert_eq!(c, 0);
}

#[test]
fn alias_with_arg_keeps_following_words() {
    let (o, _, _) = run_file("shopt -s expand_aliases\nalias ll='echo LL'\nll /usr\n");
    assert_eq!(o, "LL /usr\n");
}

#[test]
fn not_expanded_without_shopt() {
    // default (no expand_aliases) in non-interactive mode: alias NOT expanded.
    let (_, e, _) = run_file("alias foo='echo HELLO'\nfoo\n");
    assert!(e.contains("foo: command not found"), "stderr: {e}");
}

#[test]
fn unalias_then_use_is_command_not_found() {
    let (o, e, _) =
        run_file("shopt -s expand_aliases\nalias foo='echo HI'\nfoo\nunalias foo\nfoo\n");
    assert_eq!(o, "HI\n");
    assert!(e.contains("foo: command not found"), "stderr: {e}");
}

#[test]
fn trailing_space_continues_expansion() {
    // alias ending in space → the next word is also alias-expanded.
    let (o, _, _) = run_file("shopt -s expand_aliases\nalias a='b '\nalias b='echo'\na hi\n");
    assert_eq!(o, "hi\n");
}

#[test]
fn redefine_alias_affects_later_use() {
    let (o, _, _) =
        run_file("shopt -s expand_aliases\nalias g='echo one'\ng\nalias g='echo two'\ng\n");
    assert_eq!(o, "one\ntwo\n");
}
