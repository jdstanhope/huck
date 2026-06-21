//! Integration tests for v88 complete/compgen action expansion (M-36a).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn registration_never_errors() {
    assert_eq!(run("complete -u cmd; echo rc=$?\n").0, "rc=0\n");
    assert_eq!(run("complete -A stopped cmd; echo rc=$?\n").0, "rc=0\n");
    assert_eq!(run("complete -A setopt -A shopt cmd; echo rc=$?\n").0, "rc=0\n");
    assert_eq!(run("complete -ev cmd; echo rc=$?\n").0, "rc=0\n");
}

#[test]
fn compgen_setopt_shopt_signal() {
    assert_eq!(run("compgen -A setopt e\n").0, "emacs\nerrexit\nerrtrace\n");
    assert_eq!(run("compgen -A shopt null\n").0, "nullglob\n");
    // Use a prefix that completes to exactly one signal on every platform.
    // `SIGIN` is ambiguous on BSD/macOS, which also has SIGINFO; `SIGTER`
    // resolves to just SIGTERM on both Linux and macOS.
    assert_eq!(run("compgen -A signal SIGTER\n").0, "SIGTERM\n");
}

#[test]
fn compgen_export_arrayvar_builtin() {
    assert_eq!(run("export FOO=1\ncompgen -A export FO\n").0, "FOO\n");
    assert_eq!(run("arr=(x y)\ncompgen -A arrayvar ar\n").0, "arr\n");
    assert_eq!(run("compgen -b ec\n").0, "echo\n");
    assert!(run("compgen -v PA\n").0.lines().any(|l| l == "PATH"));
}

#[test]
fn compgen_empty_actions_rc_one_no_error() {
    // recognized-but-empty: rc 1, no stdout, no stderr-driven failure
    let (out, rc) = run("compgen -A hostname x\n");
    assert_eq!(out, "");
    assert_eq!(rc, 1);
    assert_eq!(run("compgen -A binding\n").1, 1);
}
