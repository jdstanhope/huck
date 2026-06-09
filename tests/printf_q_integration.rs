//! v120: printf %q (M-73).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v120q_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test]
fn q_simple() {
    assert_eq!(run("printf '%q\\n' 'a b'\n"), "a\\ b\n");
    assert_eq!(run("printf '%q\\n' plain\n"), "plain\n");
    assert_eq!(run("printf '%q\\n' \"c'd\"\n"), "c\\'d\n");
    assert_eq!(run("printf '[%q]\\n' ''\n"), "['']\n");
}
#[test]
fn q_control_and_cycle() {
    assert_eq!(run("printf '%q\\n' \"$(printf 'a\\tb')\"\n"), "$'a\\tb'\n");
    assert_eq!(run("printf '%q\\n' one two three\n"), "one\ntwo\nthree\n");
}
#[test]
fn q_width_and_capture() {
    assert_eq!(run("printf '[%6q]\\n' 'a b'\n"), "[  a\\ b]\n");
    assert_eq!(run("printf -v x '%q' 'a b'\necho \"$x\"\n"), "a\\ b\n");
}
