//! v147: export -f — function export/import interop + Shellshock hardening.
use std::process::{Command, Stdio};

fn huck() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(prog: &str, args: &[&str], envs: &[(&str, &str)]) -> (String, i32) {
    let mut c = Command::new(prog);
    c.args(args).stdin(Stdio::null());
    for (k, v) in envs { c.env(k, v); }
    let o = c.output().expect("spawn");
    (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
}

#[test]
fn huck_exports_to_child_bash() {
    let (out, _) = run(huck(), &["-c", "f(){ echo OK; }; export -f f; bash -c f"], &[]);
    assert_eq!(out, "OK\n", "child bash didn't import: {out:?}");
}

#[test]
fn huck_exports_to_child_huck() {
    let cmd = format!("f(){{ echo OK; }}; export -f f; {} -c f", huck());
    let (out, _) = run(huck(), &["-c", &cmd], &[]);
    assert_eq!(out, "OK\n", "child huck didn't import: {out:?}");
}

#[test]
fn huck_imports_bash_shaped_env() {
    let (out, _) = run(huck(), &["-c", "g"], &[("BASH_FUNC_g%%", "() { echo FROMBASH; }")]);
    assert_eq!(out, "FROMBASH\n", "huck didn't import BASH_FUNC env: {out:?}");
}

#[test]
fn export_f_not_a_function_rc1() {
    let (_o, rc) = run(huck(), &["-c", "export -f nope"], &[]);
    assert_eq!(rc, 1);
}

#[test]
fn unset_f_unexports() {
    let (out, _) = run(huck(),
        &["-c", "f(){ :; }; export -f f; unset -f f; env | grep -c BASH_FUNC_f || true"], &[]);
    assert_eq!(out.trim(), "0", "unset -f should drop the export: {out:?}");
}

#[test]
fn shellshock_trailing_command_not_executed() {
    let marker = format!("/tmp/huck_pwn_{}", std::process::id());
    let _ = std::fs::remove_file(&marker);
    let payload = format!("() {{ :; }}; touch {marker}");
    let (out, _rc) = run(huck(),
        &["-c", "type x >/dev/null 2>&1 && echo DEFINED || echo undefined"],
        &[("BASH_FUNC_x%%", &payload)]);
    assert!(!std::path::Path::new(&marker).exists(), "Shellshock: trailing command ran!");
    assert_eq!(out.trim(), "undefined", "malicious function should not be defined: {out:?}");
    let _ = std::fs::remove_file(&marker);
}
