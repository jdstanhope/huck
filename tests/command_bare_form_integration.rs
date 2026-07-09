//! v99: `command CMD` bare form (bypass function/alias lookup). M-85.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn command_bypasses_function_for_builtin() {
    // A function shadowing the `echo` builtin is bypassed by `command echo`;
    // the echo BUILTIN runs (prints "hi"), not the function ("FUNC").
    assert_eq!(run("echo() { printf FUNC; }\ncommand echo hi\n").0, "hi\n");
}

#[test]
fn command_bypasses_function_for_external() {
    // A function shadowing an external is bypassed; the real command runs.
    // `true(){ return 7; }` would yield rc 7, but `command true` runs the
    // real `true` (rc 0).
    assert_eq!(
        run("true() { return 7; }\ncommand true; echo rc=$?\n").0,
        "rc=0\n"
    );
}

#[test]
fn command_runs_builtin() {
    assert_eq!(run("command echo hi\n").0, "hi\n");
}

#[test]
fn command_runs_external() {
    assert_eq!(run("command printf '%s\\n' external\n").0, "external\n");
}

#[test]
fn command_double_collapses() {
    assert_eq!(run("command command echo nested\n").0, "nested\n");
}

#[test]
fn command_not_found_127() {
    assert_eq!(
        run("command no_such_cmd_xyz_123 2>/dev/null; echo rc=$?\n").0,
        "rc=127\n"
    );
}

#[test]
fn command_inline_assignment_applies() {
    // The rewritten program participates normally in && chaining.
    assert_eq!(run("command true && echo ok\n").0, "ok\n");
}

#[test]
fn command_dash_v_unchanged() {
    assert_eq!(run("command -v echo\n").0, "echo\n");
}

#[test]
fn command_no_operand_zero() {
    assert_eq!(run("command\necho rc=$?\n").0, "rc=0\n");
}

#[test]
fn command_dash_p_accepts() {
    assert_eq!(run("command -p echo hi\n").0, "hi\n");
}

#[test]
fn command_declaration_builtin_no_panic() {
    // `command export …` / `command declare …` must not panic; a scalar export
    // runs the declaration builtin and persists, exactly like bash.
    let (out, rc) = run("command export FOO=bar\necho \"[${FOO}]\"\n");
    assert_eq!(out, "[bar]\n");
    assert_ne!(rc, 101); // never a panic
}

#[test]
fn command_declaration_array_literal_rejected_like_bash() {
    // `command declare -a a=(x y z)` is a parse-time syntax error in bash (the
    // `(` after `command …` is unexpected). Since the parser-driven front-end
    // landed (v264 flip → oracle deletion → v268) huck rejects it the same way
    // (rc 2) instead of reconstructing and assigning the array — it converged to
    // bash. The must-not-panic guarantee still holds.
    let (out, rc) = run("command declare -a a=(x y z); echo \"${a[1]}\"\n");
    assert_ne!(rc, 101, "never a panic");
    assert_eq!(rc, 2, "syntax error like bash, rc: {rc}");
    assert_eq!(out, "", "no stdout, out: {out:?}");
}

#[test]
fn command_p_with_introspect() {
    // -p before -v must not spuriously error ("invalid option").
    let (out, rc) = run("command -p -v echo\n");
    assert_eq!(out, "echo\n");
    assert_eq!(rc, 0);
}

#[test]
fn command_v_then_p_with_introspect() {
    // -v before -p is also accepted.
    let (out, rc) = run("command -v -p echo\n");
    assert_eq!(out, "echo\n");
    assert_eq!(rc, 0);
}

#[test]
fn command_dashdash_no_operand() {
    assert_eq!(run("command --; echo rc=$?\n").0, "rc=0\n");
}

#[test]
fn command_echo_dash_v_is_echo_arg() {
    // `-v` here is echo's ARG, not command's flag.
    assert_eq!(run("command echo -v\n").0, "-v\n");
}

#[test]
fn command_bypass_inline_assignment_is_temporary() {
    // `command <fn-name>` runs the external/builtin; a leading inline assignment
    // must be TEMPORARY (restored), like bash — not persisted via the bypassed fn.
    let (out, _rc) = run("true() { return 0; }\nFOO=zzz command true\necho \"after=[${FOO}]\"\n");
    assert_eq!(out, "after=[]\n");
}
