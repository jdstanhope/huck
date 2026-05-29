use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn local_scopes_to_function() {
    let script = "X=outer\n\
                  f() { local X=in; echo \"in=$X\"; }\n\
                  f\n\
                  echo \"out=$X\"\n\
                  exit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "in=in"),
        "expected `in=in` in: {:?}",
        out
    );
    assert!(
        out.lines().any(|l| l == "out=outer"),
        "expected `out=outer` (locals restored) in: {:?}",
        out
    );
}

#[test]
fn local_outside_function_errors() {
    let script = "local X=1\nrc=$?\necho rc=$rc\nexit\n";
    let (out, err) = run_capture(script);
    let rc_line = out
        .lines()
        .find(|l| l.starts_with("rc="))
        .unwrap_or_else(|| panic!("no rc= line in stdout: {:?}; stderr: {:?}", out, err));
    let rc = rc_line.strip_prefix("rc=").unwrap();
    assert_ne!(rc, "0", "expected non-zero rc, got {rc}; stderr: {:?}", err);
    assert!(
        err.contains("can only be used in a function"),
        "expected stderr to mention 'can only be used in a function': {:?}",
        err
    );
}
