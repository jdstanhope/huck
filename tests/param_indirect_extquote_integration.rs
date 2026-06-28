//! v234: ${!name[sub]<modifier>} indirect-with-subscript-modifier (Feature 1)
//! and ${$'…'} extquote name (Feature 2) — parse + behave like bash.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v234_{}_{}_.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn indirect_subscript_suffix_op_scalar_degenerate() {
    // v=arr (scalar) -> v[@] is "arr" -> ${arr}=arr[0]="aa" -> %b -> "aa".
    let (o, _e, c) = run_file("v=arr; arr=(aa bb); echo \"${!v[@]%b}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "aa\n");
}

#[test]
fn indirect_subscript_transform_op() {
    let (o, _e, c) = run_file("v=arr; arr=(aa bb); echo \"${!v[@]@Q}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "'aa'\n");
}

#[test]
fn indirect_subscript_real_array_is_invalid_name() {
    // Real array: arr[@] joins to "aa bb cb", used as a name -> invalid.
    let (_o, e, c) = run_file("arr=(aa bb cb); echo \"${!arr[@]%b}\"\n");
    assert_eq!(c, 1);
    assert!(e.contains("invalid variable name"), "stderr: {e}");
}

#[test]
fn indirect_keys_bare_still_works() {
    let (o, _e, c) = run_file("arr=(aa bb cb); echo \"${!arr[@]}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "0 1 2\n");
}

#[test]
fn extquote_name_resolves_value() {
    let (o, _e, c) = run_file("x1=not; echo \"${$'x1'}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "not\n");
}

#[test]
fn extquote_nested_pattern_operand() {
    // ${x#${$'x1'%$'t'}} -> ${x1%t}="no" -> strip prefix "no" from "notOK" -> "tOK".
    let (o, _e, c) = run_file("x=notOK; x1=not; echo \"${x#${$'x1'%$'t'}}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "tOK\n");
}

#[test]
fn extquote_declare_f_reconstructs_decoded() {
    // declare -f normalizes ${$'x1'} to ${x1} (bash behavior, free via decoded name).
    let (o, _e, _c) = run_file("f() { x1=not; echo \"${$'x1'}\"; }\ndeclare -f f\n");
    assert!(o.contains("${x1}"), "stdout: {o}");
}

#[test]
fn extquote_name_honors_nounset() {
    // Regression F2: promoted ParamExpansion{None} node must honor set -u.
    // Unset var with nounset should fail (rc 1, "unbound variable" on stderr).
    let (_o, e, c) = run_file("set -u; echo \"${$'unsetvar'}\"\n");
    assert_eq!(c, 1, "expected rc=1 (nounset), got {c}; stderr: {e}");
    assert!(e.contains("unbound variable"), "expected 'unbound variable' in stderr: {e}");

    // Set var with nounset should succeed.
    let (o, _e, c) = run_file("set -u; x1=set; echo \"${$'x1'}\"\n");
    assert_eq!(c, 0, "expected rc=0 for set var, got {c}");
    assert_eq!(o, "set\n", "expected 'set', got {o:?}");
}

// === Final-review regression tests (v234 findings) ===

#[test]
fn indirect_through_array_element_ref() {
    // bash: ${!v} where v="arr[0]" resolves to arr[0] value ("aa").
    // huck PREVIOUSLY fired "invalid variable name" because brackets
    // made is_valid_name return false before split_name_subscript ran.
    let (o, e, c) = run_file("arr=(aa bb); v=\"arr[0]\"; echo \"${!v}\"\n");
    assert_eq!(c, 0, "expected rc=0 for arr[0] indirect; rc={c}; stderr={e}");
    assert_eq!(o, "aa\n", "expected 'aa', got {o:?}");

    // Index 1
    let (o, e, c) = run_file("arr=(aa bb); v=\"arr[1]\"; echo \"${!v:-d}\"\n");
    assert_eq!(c, 0, "expected rc=0 for arr[1] indirect; rc={c}; stderr={e}");
    assert_eq!(o, "bb\n", "expected 'bb', got {o:?}");

    // Associative array element
    let (o, e, c) = run_file("declare -A m=([k]=v); v=\"m[k]\"; echo \"${!v}\"\n");
    assert_eq!(c, 0, "expected rc=0 for m[k] indirect; rc={c}; stderr={e}");
    assert_eq!(o, "v\n", "expected 'v', got {o:?}");
}

// === v235: extquote gate (M-156) — in_dquote context ===

#[test]
fn extquote_pattern_quoted_decodes() {
    // Quoted outer -> the nested ${$'x1'%$'t'} in the # pattern decodes.
    let (o, _e, c) = run_file("x=notOK; x1=not; echo \"${x#${$'x1'%$'t'}}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "tOK\n");
}

#[test]
fn extquote_pattern_unquoted_is_bad_subst() {
    // Unquoted outer -> the nested extquote name is a runtime bad substitution.
    let (_o, e, c) = run_file("x=notOK; x1=not; echo ${x#${$'x1'%$'t'}}\n");
    assert_eq!(c, 1);
    assert!(e.contains("bad substitution"), "stderr: {e}");
}

#[test]
fn extquote_default_unquoted_is_bad_subst() {
    let (_o, e, c) = run_file("x1=hi; unset z; echo ${z:-${$'x1'}}\n");
    assert_eq!(c, 1);
    assert!(e.contains("bad substitution"), "stderr: {e}");
}

#[test]
fn extquote_default_quoted_decodes() {
    let (o, _e, c) = run_file("x1=hi; unset z; echo \"${z:-${$'x1'}}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "hi\n");
}
