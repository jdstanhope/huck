//! v146: `declare -f NAME` prints the real (normalized) function body via the
//! `generate` module, and the printed text re-parses + executes equivalently.
use std::process::{Command, Stdio};

fn huck_c(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg("-c")
        .arg(s)
        .stdin(Stdio::null())
        .output()
        .expect("spawn huck");
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
    assert_eq!(
        out, "1\n2\n3\n",
        "round-tripped function changed behavior: {out:?}"
    );
}

#[test]
fn declare_f_reparse_executes_if_case() {
    let out = huck_c(
        "h(){ case \"$1\" in a) echo A;; *) echo other;; esac; }; eval \"$(declare -f h)\"; h a; h z",
    );
    assert_eq!(
        out, "A\nother\n",
        "if/case round-trip changed behavior: {out:?}"
    );
}

#[test]
fn declare_f_glob_arg_still_globs() {
    // A glob in a function body must STILL glob after declare -f round-trip.
    let out = huck_c(
        "cd /tmp && mkdir -p hgt && : >hgt/sa && : >hgt/sb && \
         f(){ cd /tmp/hgt && echo s*; }; \
         direct=$(f); eval \"$(declare -f f)\"; round=$(f); \
         rm -rf /tmp/hgt; \
         [ \"$direct\" = \"$round\" ] && [ \"$direct\" = \"sa sb\" ] && echo MATCH || echo \"DIFF d=[$direct] r=[$round]\"",
    );
    assert!(
        out.contains("MATCH"),
        "glob lost across declare -f: {out:?}"
    );
}

#[test]
fn declare_f_escaped_glob_stays_literal() {
    // An ESCAPED star must STAY literal across round-trip (must NOT start globbing).
    let out = huck_c(
        "cd /tmp && mkdir -p hgt2 && : >hgt2/sa && \
         g(){ cd /tmp/hgt2 && echo s\\*; }; \
         direct=$(g); eval \"$(declare -f g)\"; round=$(g); \
         rm -rf /tmp/hgt2; \
         [ \"$direct\" = \"$round\" ] && [ \"$direct\" = \"s*\" ] && echo MATCH || echo \"DIFF d=[$direct] r=[$round]\"",
    );
    assert!(
        out.contains("MATCH"),
        "escaped star changed meaning: {out:?}"
    );
}

#[test]
fn declare_f_test_builtin_clean() {
    // The `[` builtin should render cleanly (not `\[`) and re-parse + run.
    let out = huck_c("f(){ if [ -n \"$1\" ]; then echo yes; fi; }; declare -f f");
    assert!(!out.contains("\\["), "test builtin over-escaped: {out:?}");
    let run = huck_c("f(){ if [ -n \"$1\" ]; then echo yes; fi; }; eval \"$(declare -f f)\"; f x");
    assert_eq!(run, "yes\n", "re-parsed [ test broke: {run:?}");
}

#[test]
fn declare_f_missing_silent() {
    // bash: declare -f on a missing function prints nothing.
    let out = huck_c("declare -f nosuchfn");
    assert_eq!(out, "", "should be silent: {out:?}");
}

#[test]
fn declare_f_preserves_definition_redirect() {
    // v187 (M-09b): a definition-attached redirect renders in declare -f and
    // round-trips through eval.
    let d = huck_c("f() { echo hi; } >&2; declare -f f");
    assert!(d.contains("&2"), "declare -f dropped the redirect: {d:?}");
    let out = huck_c("f() { echo hi; } >&2; eval \"$(declare -f f)\"; f 2>&1");
    assert!(
        out.contains("hi"),
        "redirect not preserved through round-trip: {out:?}"
    );
}
