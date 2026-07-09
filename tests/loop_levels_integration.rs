//! Integration tests for v79 break N / continue N loop levels.
//! Drives the `huck` binary via stdin and asserts on stdout/exit code.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
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
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn break_2_in_nested_for_exits_both() {
    let script = r#"for i in 1 2; do for j in a b; do echo "$i$j"; break 2; done; done
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "1a\n");
}

#[test]
fn continue_2_in_nested_for_advances_outer() {
    let script = r#"for i in 1 2 3; do for j in a b; do if [ "$j" = "b" ]; then continue 2; fi; echo "$i$j"; done; done
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    // For each outer i, the inner iterates j=a (printed), then j=b
    // triggers continue 2 (skips rest of inner; advances outer).
    assert_eq!(out, "1a\n2a\n3a\n");
}

#[test]
fn break_overshoot_caps_to_depth() {
    let script = "for i in 1; do break 999; done; echo ok\n";
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "ok\n");
}

#[test]
fn break_outside_any_loop_errors() {
    let (out, err, code) = run_huck("break\necho $?\n");
    // The break errors with status 0 (bash compat), script continues to
    // the next command (echo). Final exit code is 0 (echo's status).
    assert_eq!(code, 0);
    // $? after break outside a loop is 0 (bash 5.2 behavior).
    assert_eq!(out, "0\n");
    assert!(
        err.contains("only meaningful in a"),
        "stderr should mention 'only meaningful': {err:?}",
    );
}

#[test]
fn break_inside_function_called_from_loop_errors() {
    let script = r#"f() { break; }
for i in 1; do f; done
echo ok
"#;
    let (out, err, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "ok\n");
    assert!(err.contains("only meaningful"), "stderr: {err:?}");
}

#[test]
fn mixed_for_while_break_2() {
    let script = r#"i=0
for outer in 1 2 3; do
    while [ "$i" -lt 5 ]; do
        i=$((i+1))
        if [ "$i" -ge 2 ]; then break 2; fi
    done
done
echo "i=$i"
"#;
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "i=2\n");
}
