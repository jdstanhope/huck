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
fn shift_advances_positional_in_function() {
    let script = "f() { shift; echo $1; }\nf a b c\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "b"),
        "expected line `b` (the shifted $1) in: {:?}",
        out
    );
}

#[test]
fn set_then_for_loop_positional() {
    let script = "set -- one two three\nfor arg in \"$@\"; do echo $arg; done\nexit\n";
    let (out, _) = run_capture(script);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"one"), "missing `one` in: {:?}", out);
    assert!(lines.contains(&"two"), "missing `two` in: {:?}", out);
    assert!(lines.contains(&"three"), "missing `three` in: {:?}", out);
}
