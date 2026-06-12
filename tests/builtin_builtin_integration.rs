//! v142: the `builtin NAME [args]` builtin — runs the named shell builtin directly,
//! bypassing functions/aliases; errors if NAME is not a builtin. Fixes mise's
//! `cd(){ builtin cd "$@"; }` wrapper.
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn huck_c(script: &str) -> (String, String, i32) {
    let o = Command::new(huck_bin())
        .arg("-c").arg(script)
        .stdin(Stdio::null())
        .output()
        .expect("spawn huck");
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

#[test]
fn builtin_echo() {
    let (out, _e, code) = huck_c("builtin echo hi");
    assert_eq!(out, "hi\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn builtin_cd_runs_cd() {
    let (out, _e, code) = huck_c("builtin cd /tmp; pwd");
    assert_eq!(out, "/tmp\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn builtin_not_a_builtin_errors() {
    let (_o, err, code) = huck_c("builtin nosuchthing");
    assert!(err.contains("builtin: nosuchthing: not a shell builtin"), "err={err:?}");
    assert_eq!(code, 1);
}

#[test]
fn builtin_alone_is_noop() {
    let (out, _e, code) = huck_c("builtin; echo done");
    assert_eq!(out, "done\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn builtin_cd_wrapper_no_recursion() {
    let (out, _e, code) = huck_c(r#"cd(){ builtin cd "$@"; }; cd /tmp; pwd"#);
    assert_eq!(out, "/tmp\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn builtin_bypasses_cd_function() {
    let (out, _e, _c) = huck_c(r#"cd(){ echo SHADOW; }; builtin cd /tmp; pwd"#);
    assert_eq!(out, "/tmp\n", "out={out:?}");
}

#[test]
fn builtin_declaration_local() {
    let (out, _e, _c) = huck_c(r#"f(){ builtin local x=5; echo "$x"; }; f"#);
    assert_eq!(out, "5\n", "out={out:?}");
}

#[test]
fn type_recognizes_builtin() {
    let (out, _e, _c) = huck_c("type builtin");
    assert!(out.contains("builtin is a shell builtin"), "out={out:?}");
}
