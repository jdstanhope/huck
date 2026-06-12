//! v146: `declare -f NAME` prints the real (normalized) function body via the
//! `generate` module, and the printed text re-parses + executes equivalently.
use std::process::{Command, Stdio};

fn huck_c(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg("-c").arg(s).stdin(Stdio::null()).output().expect("spawn huck");
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn declare_f_prints_body() {
    let out = huck_c("f(){ echo hi; }; declare -f f");
    assert!(out.contains("echo hi"), "body not printed: {out:?}");
    assert_ne!(out.trim(), "declare -f f", "still the stub: {out:?}");
}

#[test]
fn declare_f_reparse_executes_for_loop() {
    let out = huck_c("g(){ for x in 1 2 3; do echo $x; done; }; eval \"$(declare -f g)\"; g");
    assert_eq!(out, "1\n2\n3\n", "round-tripped function changed behavior: {out:?}");
}

#[test]
fn declare_f_reparse_executes_if_case() {
    let out = huck_c(
        "h(){ case \"$1\" in a) echo A;; *) echo other;; esac; }; eval \"$(declare -f h)\"; h a; h z",
    );
    assert_eq!(out, "A\nother\n", "if/case round-trip changed behavior: {out:?}");
}

#[test]
fn declare_f_missing_silent() {
    // bash: declare -f on a missing function prints nothing.
    let out = huck_c("declare -f nosuchfn");
    assert_eq!(out, "", "should be silent: {out:?}");
}
