//! v95: ${!var} indirect parameter expansion (M-indirect).
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
fn indirect_through_named_var() {
    assert_eq!(run("x=hi\nref=x\necho \"${!ref}\"\n").0, "hi\n");
}

#[test]
fn indirect_through_positional() {
    // OPTIND=2 -> ${2}
    assert_eq!(run("set -- a b c\nOPTIND=2\necho \"${!OPTIND}\"\n").0, "b\n");
}

#[test]
fn indirect_with_default_modifier_unset() {
    assert_eq!(run("ref=missing\necho \"${!ref-fallback}\"\n").0, "fallback\n");
}

#[test]
fn indirect_with_default_modifier_set() {
    assert_eq!(run("x=val\nref=x\necho \"${!ref-fallback}\"\n").0, "val\n");
}

#[test]
fn indirect_source_unset_is_empty() {
    assert_eq!(run("unset ref\necho \"[${!ref}]\"\n").0, "[]\n");
}

#[test]
fn array_keys_still_work() {
    // Regression: ${!a[@]} is array keys, NOT indirect.
    assert_eq!(run("a=(p q r)\necho \"${!a[@]}\"\n").0, "0 1 2\n");
}
