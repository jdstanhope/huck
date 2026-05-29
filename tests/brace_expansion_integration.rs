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
fn brace_list_in_echo() {
    let (out, _) = run_capture("echo {a,b,c}\nexit\n");
    assert!(
        out.lines().any(|l| l == "a b c"),
        "expected `a b c` line in: {:?}",
        out
    );
}

#[test]
fn brace_range_in_for_loop() {
    let (out, _) = run_capture("for i in {1..3}; do echo \"i=$i\"; done\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"i=1"), "missing i=1 in: {:?}", out);
    assert!(lines.contains(&"i=2"), "missing i=2 in: {:?}", out);
    assert!(lines.contains(&"i=3"), "missing i=3 in: {:?}", out);
}

#[test]
fn brace_cartesian() {
    let (out, _) = run_capture(
        "for d in /tmp/{a,b}/{x,y}; do echo $d; done\nexit\n",
    );
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"/tmp/a/x"), "missing /tmp/a/x in: {:?}", out);
    assert!(lines.contains(&"/tmp/a/y"), "missing /tmp/a/y in: {:?}", out);
    assert!(lines.contains(&"/tmp/b/x"), "missing /tmp/b/x in: {:?}", out);
    assert!(lines.contains(&"/tmp/b/y"), "missing /tmp/b/y in: {:?}", out);
}
