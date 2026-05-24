use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn pipeline_stage_inline_assignments_are_scoped_per_stage() {
    let (out, _) = run("FOO=stage1 env | FOO=stage2 grep ^FOO=\n");
    // grep's env has FOO=stage2; stage1's env had FOO=stage1.
    // `env` outputs all its env vars; `grep` filters to ^FOO=.
    // The line we expect is `FOO=stage1` because that's what `env`
    // saw — `grep` sees its own FOO=stage2 but only prints from stdin.
    assert!(out.contains("FOO=stage1"), "got: {out}");
    assert!(!out.contains("FOO=stage2"), "stage2 should not leak: {out}");
}
