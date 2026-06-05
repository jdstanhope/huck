//! v96: ${var@OP} parameter transforms (M-86, scalar subset).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn transform_upper() { assert_eq!(run("v=hello\necho \"${v@U}\"\n").0, "HELLO\n"); }

#[test]
fn transform_lower() { assert_eq!(run("v=HeLLo\necho \"${v@L}\"\n").0, "hello\n"); }

#[test]
fn transform_upper_first() { assert_eq!(run("v=hello\necho \"${v@u}\"\n").0, "Hello\n"); }

#[test]
fn transform_quote_simple() { assert_eq!(run("v='a b'\necho \"${v@Q}\"\n").0, "'a b'\n"); }

#[test]
fn transform_quote_unset_is_empty() {
    // bash: ${unset@Q} -> empty (no quotes); set-empty -> ''
    assert_eq!(run("unset v\nprintf '[%s]\\n' \"${v@Q}\"\n").0, "[]\n");
    assert_eq!(run("v=\nprintf '[%s]\\n' \"${v@Q}\"\n").0, "['']\n");
}

#[test]
fn transform_escape_expand() {
    // v='a\tb' (literal backslash-t) -> @E expands to a<TAB>b
    assert_eq!(run("v='a\\tb'\necho \"${v@E}\"\n").0, "a\tb\n");
}

#[test]
fn transform_prompt_expand_literal() {
    // \n in @P expands to a newline; no env-dependent escapes here.
    assert_eq!(run("v='x\\ny'\necho \"${v@P}\"\n").0, "x\ny\n");
}

#[test]
fn transform_unknown_operator_errors() {
    let (out, rc) = run("v=x\necho \"${v@Z}\"\n");
    assert_ne!(rc, 0, "unknown @-operator should error; out={out:?}");
}
