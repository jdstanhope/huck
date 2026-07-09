//! v107: bashrc builtin gaps. Task 1: `[[ -o optname ]]` option test.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Returns (stdout, stderr, exit_code).
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn dbracket_o_option_off() {
    assert_eq!(run("[[ -o emacs ]] && echo on || echo off\n").0, "off\n");
}
#[test]
fn dbracket_o_option_on() {
    assert_eq!(
        run("set -o pipefail\n[[ -o pipefail ]] && echo on || echo off\n").0,
        "on\n"
    );
}
#[test]
fn dbracket_o_reflects_errexit_unknown_negation() {
    assert_eq!(
        run("set -e\n[[ -o errexit ]] && echo on || echo off\n").0,
        "on\n"
    );
    assert_eq!(
        run("[[ -o bogusname ]] && echo on || echo off\n").0,
        "off\n"
    );
    assert_eq!(run("[[ ! -o pipefail ]] && echo y || echo n\n").0, "y\n");
}
#[test]
fn dbracket_o_git_prompt_shape() {
    // Matches bash: ZSH_VERSION is unset so `[ -z "" ]` is true and the `||`
    // chain short-circuits with nothing printed. The `[[ -o PROMPT_SUBST ]]`
    // arm is the load-bearing part — it must parse, not fail "unterminated".
    assert_eq!(
        run("[ -z \"${ZSH_VERSION-}\" ] || [[ -o PROMPT_SUBST ]] || echo fallback\n").0,
        ""
    );
    // When the first guard is false, the `[[ -o PROMPT_SUBST ]]` (unknown/off)
    // is reached and is false, so `echo fallback` runs — like bash.
    assert_eq!(
        run(
            "ZSH_VERSION=1\n[ -z \"${ZSH_VERSION-}\" ] || [[ -o PROMPT_SUBST ]] || echo fallback\n"
        )
        .0,
        "fallback\n"
    );
}

#[test]
fn declare_g_survives_function_exit() {
    assert_eq!(
        run("f() { declare -g G=1; }\nf\necho \"[${G-}]\"\n").0,
        "[1]\n"
    );
}
#[test]
fn declare_without_g_is_local() {
    assert_eq!(run("f() { declare L=1; }\nf\necho \"[${L-}]\"\n").0, "[]\n");
}
#[test]
fn declare_g_toplevel_and_composed() {
    assert_eq!(run("declare -g X=2\necho \"$X\"\n").0, "2\n");
    assert_eq!(run("f() { declare -gx E=7; }\nf\necho \"$E\"\n").0, "7\n");
}

#[test]
fn unset_f_removes_function() {
    assert_eq!(
        run("f() { echo hi; }\nunset -f f\ntype f >/dev/null 2>&1 && echo found || echo gone\n").0,
        "gone\n"
    );
}
#[test]
fn unset_v_removes_variable() {
    assert_eq!(run("v=1\nunset -v v\necho \"[${v-}]\"\n").0, "[]\n");
}
#[test]
fn unset_missing_is_success() {
    assert_eq!(run("unset -f NOPE_FN\n").2, 0);
}
#[test]
fn unset_f_does_not_touch_samename_var() {
    assert_eq!(
        run("x=VAR\nx() { :; }\nunset -f x\necho \"$x\"\n").0,
        "VAR\n"
    );
}
