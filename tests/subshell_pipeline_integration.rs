//! Non-pty equivalence tests for M-104: subshell-internal pipelines must
//! produce byte-identical output to bash in script/piped-stdin mode (the path
//! that already worked) — guarding against any regression from the tty fix.

use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

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
fn subshell_pipeline_output_unchanged() {
    assert_eq!(run("( echo hi | cat )\necho done\n").0, "hi\ndone\n");
}

#[test]
fn subshell_multistage_pipeline_output() {
    assert_eq!(run("( printf 'a\\nb\\nc\\n' | head -n 2 | tail -n 1 )\n").0, "b\n");
}

#[test]
fn subshell_pipeline_in_command_sub() {
    assert_eq!(run("x=$( ( echo hi | cat ) ); echo \"[$x]\"\n").0, "[hi]\n");
}

#[test]
fn pipestatus_inside_subshell() {
    assert_eq!(run("( false | true ); ( true | false ); echo done\n").2, 0);
}
