//! Integration tests for v82 script-file mode + `-c`.
use std::io::Write;
use std::process::{Command, Stdio};

const HUCK: &str = env!("CARGO_BIN_EXE_huck");

/// Run huck with CLI args + optional stdin. Returns (stdout, stderr, exit).
fn run(args: &[&str], stdin: &str) -> (String, String, i32) {
    let mut child = Command::new(HUCK)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
        o.status.code().unwrap_or(-1),
    )
}

fn write_script(body: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(body.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn c_mode_first_operand_is_argv0() {
    let (out, _e, c) = run(&["-c", "echo \"0=$0 1=$1 #=$#\"", "name", "a", "b"], "");
    assert_eq!(out, "0=name 1=a #=2\n");
    assert_eq!(c, 0);
}

#[test]
fn c_mode_no_operands_argv0_is_shell_name() {
    let (out, _e, _c) = run(&["-c", "echo \"0=$0 1=$1 #=$#\""], "");
    assert!(
        out.starts_with("0=") && out.contains("huck"),
        "expected shell name in $0: {out:?}"
    );
    assert!(out.contains("#=0"), "expected #=0: {out:?}");
}

#[test]
fn c_mode_multistatement_and_exit_code() {
    let (out, _e, c) = run(&["-c", "x=5; echo $x; exit 3"], "");
    assert_eq!(out, "5\n");
    assert_eq!(c, 3);
}

#[test]
fn c_mode_multiline() {
    let (out, _e, _c) = run(&["-c", "for i in 1 2 3\ndo echo $i\ndone"], "");
    assert_eq!(out, "1\n2\n3\n");
}

#[test]
fn c_mode_empty_command_exit_zero() {
    let (out, _e, c) = run(&["-c", ""], "");
    assert_eq!(out, "");
    assert_eq!(c, 0);
}

#[test]
fn file_mode_argv0_and_positionals() {
    let f = write_script("echo \"0=$0 1=$1 #=$#\"\n");
    let path = f.path().to_str().unwrap();
    let (out, _e, c) = run(&[path, "x", "y"], "");
    assert_eq!(out, format!("0={path} 1=x #=2\n"));
    assert_eq!(c, 0);
}

#[test]
fn file_mode_multiline_and_exit() {
    let f = write_script("greet() { echo \"hi $1\"; }\ngreet world\nexit 4\n");
    let (out, _e, c) = run(&[f.path().to_str().unwrap()], "");
    assert_eq!(out, "hi world\n");
    assert_eq!(c, 4);
}

#[test]
fn file_mode_shebang_line_ignored() {
    let f = write_script("#!/usr/bin/env huck\necho ok\n");
    let (out, _e, c) = run(&[f.path().to_str().unwrap()], "");
    assert_eq!(out, "ok\n");
    assert_eq!(c, 0);
}

#[test]
fn file_mode_missing_file_exits_127() {
    let (_o, err, c) = run(&["/no/such/huck/script-xyz"], "");
    assert_eq!(c, 127);
    assert!(err.contains("No such file or directory"), "stderr: {err:?}");
}

#[test]
fn set_e_propagates_failure_exit() {
    let f = write_script("set -e\nfalse\necho nope\n");
    let (out, _e, c) = run(&[f.path().to_str().unwrap()], "");
    assert!(
        !out.contains("nope"),
        "errexit should stop before echo: {out:?}"
    );
    assert_ne!(c, 0, "errexit should exit non-zero");
}

#[test]
fn double_dash_routes_to_file_execution() {
    // `--` routes remaining operands to file-execution mode (the dash-leading
    // parse case is covered by the cli_double_dash_ends_options_for_file unit test).
    let f = write_script("echo viadashdash\n");
    let (out, _e, c) = run(&["--", f.path().to_str().unwrap()], "");
    assert_eq!(out, "viadashdash\n");
    assert_eq!(c, 0);
}

#[test]
fn payoff_read_from_file_consumes_real_stdin() {
    // The M-72/L-12 win: program is the file, so `read` gets real stdin.
    let f = write_script("read x\necho \"got=$x\"\n");
    let (out, _e, _c) = run(&[f.path().to_str().unwrap()], "hello\n");
    assert_eq!(out, "got=hello\n");
}

#[test]
fn set_u_aborts_script_on_unbound_var() {
    let f = write_script("set -u\necho \"x=$UNSET_VAR_XYZ\"\necho after\n");
    let (out, _e, c) = run(&[f.path().to_str().unwrap()], "");
    assert!(
        !out.contains("after"),
        "set -u should abort before 'after': {out:?}"
    );
    assert_ne!(c, 0, "set -u unbound var should exit non-zero");
}
