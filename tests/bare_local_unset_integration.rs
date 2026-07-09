//! v115: bare `local NAME` declares an unset local (M-111).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
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
fn bare_local_is_unset_for_v_test() {
    let (out, _e, _c) = run("f(){ local x; [[ -v x ]] && echo SET || echo UNSET; }\nf\n");
    assert_eq!(out, "UNSET\n", "out: {out}");
}

#[test]
fn bare_local_uses_default_in_non_colon_modifier() {
    let (out, _e, _c) = run("f(){ local x; echo \"[${x-DEF}]\"; }\nf\n");
    assert_eq!(out, "[DEF]\n", "out: {out}");
}

#[test]
fn local_explicit_empty_is_set() {
    let (out, _e, _c) = run("f(){ local x=; [[ -v x ]] && echo SET || echo UNSET; }\nf\n");
    assert_eq!(out, "SET\n", "out: {out}");
}

#[test]
fn local_with_value_is_set() {
    let (out, _e, _c) = run("f(){ local x=v; echo \"$x\"; }\nf\n");
    assert_eq!(out, "v\n", "out: {out}");
}

#[test]
fn bare_local_then_assign_is_local_and_restores_outer() {
    let (out, _e, _c) = run("x=outer\nf(){ local x; x=5; echo \"in=$x\"; }\nf\necho \"out=$x\"\n");
    assert_eq!(out, "in=5\nout=outer\n", "out: {out}");
}

#[test]
fn bare_local_shadows_outer_as_unset() {
    let (out, _e, _c) = run("x=outer\nf(){ local x; echo \"[${x-DEF}]\"; }\nf\n");
    assert_eq!(out, "[DEF]\n", "out: {out}");
}

#[test]
fn get_comp_words_local_v_shape() {
    // The bash_completion shape: bare `local … vcword` then a `[[ -v vcword ]]`
    // gate must be FALSE so no empty arg is appended.
    let (out, _e, _c) = run("f(){ local upvars=() vcur vcword\n\
           vcur=cur\n\
           [[ -v vcur ]] && upvars+=(\"$vcur\")\n\
           [[ -v vcword ]] && upvars+=(\"$vcword\")\n\
           echo \"n=${#upvars[@]} [${upvars[*]}]\"\n\
         }\nf\n");
    assert_eq!(out, "n=1 [cur]\n", "out: {out}");
}

#[test]
fn re_local_of_already_set_local_preserves_value() {
    // bash: a bare `local x` after `local x=v` in the SAME function keeps v
    // (only a FRESH bare local is unset). Regression guard for the
    // already_local gate.
    let (out, _e, _c) =
        run("f(){ local x=v; local x; [[ -v x ]] && echo \"SET=[$x]\" || echo UNSET; }\nf\n");
    assert_eq!(out, "SET=[v]\n", "out: {out}");
}
