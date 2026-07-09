//! Integration tests for v90 extglob string matching (M-84).
use std::io::Write;
use std::process::{Command, Stdio};
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
fn run(script: &str) -> (String, i32) {
    let mut c = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    c.stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let o = c.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

#[test]
fn param_expansion_extglob() {
    assert_eq!(
        run("shopt -s extglob\nv=aaab\necho \"${v##+(a)}\"\n").0,
        "b\n"
    );
    assert_eq!(
        run("shopt -s extglob\nv=foobarbar\necho \"${v%%+(bar)}\"\n").0,
        "foo\n"
    );
    assert_eq!(
        run("shopt -s extglob\nv=abcabc\necho \"${v/+(abc)/X}\"\n").0,
        "X\n"
    );
}

#[test]
fn param_expansion_extglob_off_is_literal() {
    // extglob off: `+(a)` is a literal pattern, no strip (matches bash).
    assert_eq!(run("v=aaab\necho \"${v##+(a)}\"\n").0, "aaab\n");
}

#[test]
fn dbracket_extglob_parses_when_enabled() {
    // After Task 2 the pattern lexes as one word, so [[ no longer syntax-errors.
    // (Matching is wired in Task 3; here we only assert it PARSES + runs.)
    let (_, rc) = run("shopt -s extglob\n[[ x == +(a|b) ]]\necho done\n");
    assert_eq!(rc, 0); // no syntax error; `echo done` ran
    assert_eq!(
        run("shopt -s extglob\n[[ x == +(a|b) ]]; echo done\n").0,
        "done\n"
    );
}

#[test]
fn dbracket_extglob_matches() {
    assert_eq!(
        run("shopt -s extglob\n[[ aab == +(a|b) ]] && echo y || echo n\n").0,
        "y\n"
    );
    assert_eq!(
        run("shopt -s extglob\n[[ abcd == @(ab|cd) ]] && echo y || echo n\n").0,
        "n\n"
    );
    assert_eq!(
        run("shopt -s extglob\n[[ foo == !(bar) ]] && echo y || echo n\n").0,
        "y\n"
    );
    assert_eq!(
        run("shopt -s extglob\n[[ bar == !(bar) ]] && echo y || echo n\n").0,
        "n\n"
    );
}

#[test]
fn case_extglob_matches() {
    assert_eq!(
        run("shopt -s extglob\ncase hello in +([a-z])) echo lc;; *) echo o;; esac\n").0,
        "lc\n"
    );
    assert_eq!(
        run("shopt -s extglob\ncase ab in @(a|b)) echo one;; *) echo o;; esac\n").0,
        "o\n"
    ); // ab != one-of a|b
}

#[test]
fn dbracket_extglob_nocasematch() {
    assert_eq!(
        run("shopt -s extglob nocasematch\n[[ AAB == +(a|b) ]] && echo y || echo n\n").0,
        "y\n"
    );
}

#[test]
fn extglob_group_expands_inner_variable() {
    assert_eq!(
        run("shopt -s extglob\nx=\"a|b\"\n[[ ab == +($x) ]] && echo Y || echo N\n").0,
        "Y\n"
    );
    assert_eq!(
        run("shopt -s extglob\nx=\"a|b\"\ncase ab in +($x)) echo Y;; *) echo N;; esac\n").0,
        "Y\n"
    );
    assert_eq!(
        run("shopt -s extglob\np=ab\n[[ xaby == x@($p)y ]] && echo Y || echo N\n").0,
        "Y\n"
    );
}

#[test]
fn extglob_quoted_metachars_are_literal() {
    // A `|`/`(`/`)` typed literally inside quotes inside an extglob group must
    // be literal, not alternation/group syntax (regression guard).
    assert_eq!(
        run("shopt -s extglob\n[[ a == @(\"a|b\") ]] && echo Y || echo N\n").0,
        "N\n"
    );
    assert_eq!(
        run("shopt -s extglob\n[[ 'a|b' == @(\"a|b\") ]] && echo Y || echo N\n").0,
        "Y\n"
    );
    assert_eq!(
        run("shopt -s extglob\n[[ 'a)b' == @(\"a)b\") ]] && echo Y || echo N\n").0,
        "Y\n"
    );
}
