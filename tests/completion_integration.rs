//! Integration tests for v76 programmable completion. Drives the
//! `huck` binary via stdin and asserts on stdout/exit code. Tab
//! completion proper requires an interactive tty, so these tests use
//! `compgen` exclusively — which exercises the same `resolve_spec`
//! pipeline as Tab.
//
// Several test names embed bash flag letters as uppercase (`-F`, `-P`,
// `-S`, `-X`, `-D`) for legibility; suppress the snake-case lint.
#![allow(non_snake_case)]

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn compgen_wordlist_basic() {
    let (out, _, code) = run_huck(r#"compgen -W "alpha alpine beta" -- al"#);
    assert_eq!(code, 0);
    assert_eq!(out, "alpha\nalpine\n");
}

#[test]
fn compgen_wordlist_no_match_exits_1() {
    let (out, _, code) = run_huck(r#"compgen -W "alpha beta" -- z"#);
    assert_eq!(code, 1);
    assert!(out.is_empty());
}

#[test]
fn compgen_action_builtin() {
    let (out, _, code) = run_huck(r#"compgen -A builtin -- ec"#);
    assert_eq!(code, 0);
    assert!(out.lines().any(|l| l == "echo"), "{out:?}");
}

#[test]
fn compgen_action_function() {
    let script = r#"
_alpha() { :; }
_alpine() { :; }
_beta() { :; }
compgen -A function -- _al
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    let names: Vec<&str> = out.lines().collect();
    assert_eq!(names, vec!["_alpha", "_alpine"]);
}

#[test]
fn complete_p_round_trip() {
    let script = r#"
complete -W "alpha apple banana" -- myc
complete -p myc
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert!(out.contains("complete"));
    assert!(out.contains("-W"));
    assert!(out.contains("alpha apple banana"));
    assert!(out.contains("-- myc"));
}

#[test]
fn complete_r_removes() {
    let script = r#"
complete -W "x" -- foo
complete -p foo
complete -r foo
complete -p foo
"#;
    let (_out, _err, code) = run_huck(script);
    // The last `complete -p foo` fails (status 1) since the spec was
    // removed. We only check the OVERALL exit isn't 0 -- but in
    // non-interactive mode the shell exits with the LAST status.
    assert_eq!(code, 1);
}

#[test]
fn compgen_F_function_invocation() {
    let script = r#"
_myf() { COMPREPLY=(alpha beta gamma); }
compgen -F _myf -- ""
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "alpha\nbeta\ngamma\n");
}

#[test]
fn compgen_F_function_reads_dollar_args() {
    let script = r#"
_myf() { COMPREPLY=("$1" "$2"); }
compgen -F _myf -- prefix
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    // $1 = compgen (cmd_name passed to -F context for compgen is the
    // builtin's own name), $2 = "prefix".
    assert_eq!(out, "compgen\nprefix\n");
}

#[test]
fn compgen_P_prefix_decorates() {
    let (out, _, code) = run_huck(r#"compgen -W "a b" -P "x:" -- """#);
    assert_eq!(code, 0);
    assert_eq!(out, "x:a\nx:b\n");
}

#[test]
fn compgen_S_suffix_decorates() {
    let (out, _, code) = run_huck(r#"compgen -W "a b" -S ":y" -- """#);
    assert_eq!(code, 0);
    assert_eq!(out, "a:y\nb:y\n");
}

#[test]
fn compgen_X_filter_removes() {
    let (out, _, code) = run_huck(r#"compgen -W "alpha apple banana cherry" -X "a*" -- """#);
    assert_eq!(code, 0);
    assert_eq!(out, "banana\ncherry\n");
}

#[test]
fn compgen_X_bang_keeps_only() {
    // Use single quotes around `!a*` to avoid huck's eager history
    // expansion on the `!`. (huck currently runs history expansion
    // unconditionally on every read line; bash gates it on
    // interactivity. Unrelated to the -X behavior under test.)
    let (out, _, code) = run_huck(r#"compgen -W "alpha apple banana cherry" -X '!a*' -- """#);
    assert_eq!(code, 0);
    assert_eq!(out, "alpha\napple\n");
}

#[test]
fn compopt_named_persists() {
    let script = r#"
complete -W "x" -- foo
compopt -o nospace foo
complete -p foo
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert!(out.contains("-o nospace"), "{out:?}");
}

#[test]
fn complete_D_registers_default() {
    let script = r#"
complete -D -W "dflt"
complete -p
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert!(out.contains("-D"), "{out:?}");
    assert!(out.contains("dflt"), "{out:?}");
}

#[test]
fn complete_invalid_action_exits_2() {
    let (_out, err, code) = run_huck(r#"complete -A hostname -- foo"#);
    assert_eq!(code, 2);
    assert!(err.contains("invalid action"), "{err:?}");
}
