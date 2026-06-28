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
