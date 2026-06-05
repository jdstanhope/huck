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
fn indirect_unset_source_errors() {
    // bash: empty through-value -> "invalid indirect expansion", fatal (rc != 0),
    // no normal output. (NOT empty-string.)
    let (out, rc) = run("unset ref\necho \"[${!ref}]\"\necho AFTER\n");
    assert_ne!(rc, 0, "expected nonzero exit; out={out:?}");
    assert!(!out.contains("[]"), "should not emit empty indirect result; out={out:?}");
}

#[test]
fn indirect_effective_name_unbound_under_set_u() {
    let (out, rc) = run("set -u\nref=missingvar\necho \"[${!ref}]\"\necho AFTER\n");
    assert_ne!(rc, 0, "expected nonzero exit under set -u; out={out:?}");
}

#[test]
fn indirect_effective_name_unset_no_u_is_empty() {
    // No set -u: effective name is valid but unset -> empty, like ${missingvar}.
    assert_eq!(run("ref=missingvar\necho \"[${!ref}]\"\n").0, "[]\n");
}

#[test]
fn array_keys_still_work() {
    // Regression: ${!a[@]} is array keys, NOT indirect.
    assert_eq!(run("a=(p q r)\necho \"${!a[@]}\"\n").0, "0 1 2\n");
}

#[test]
fn indirect_spaced_source_value_is_not_trimmed() {
    // bash: a through-value with surrounding spaces is an invalid name -> empty,
    // NOT resolved to the trimmed name. (Verbatim, no trim.)
    assert_eq!(run("x=hi\nref=\" x \"\necho \"[${!ref}]\"\n").0, "[]\n");
}
