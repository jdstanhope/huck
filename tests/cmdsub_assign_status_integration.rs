//! v126: a bare assignment's $? = the last command substitution's exit status
//! in its RHS (or 0 if none). File-arg execution (L-27).

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

fn huck_stdout(frag: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "huck_v126_{}_{}.sh",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let mut f = std::fs::File::create(&path).expect("create temp script");
    f.write_all(frag.as_bytes()).expect("write temp script");
    drop(f);
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg(&path)
        .output()
        .expect("run huck");
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

#[test]
fn cmdsub_false_status() {
    assert_eq!(huck_stdout("x=$(false); echo $?"), "1");
}
#[test]
fn cmdsub_exit7_status() {
    assert_eq!(huck_stdout("x=$(exit 7); echo $?"), "7");
}
#[test]
fn plain_assign_zero() {
    assert_eq!(huck_stdout("x=5; echo $?"), "0");
}
#[test]
fn two_assigns_last_wins() {
    assert_eq!(huck_stdout("x=$(false) y=$(exit 2); echo $?"), "2");
}
#[test]
fn two_subs_one_rhs_last_wins() {
    assert_eq!(huck_stdout(r#"x="$(false)$(exit 5)"; echo $?"#), "5");
}
#[test]
fn dollar_question_in_rhs_reads_previous_status() {
    assert_eq!(huck_stdout("false; x=$?; echo $x"), "1");
}
#[test]
fn local_assign_keeps_builtin_status() {
    assert_eq!(huck_stdout("f(){ local v=$(exit 9); echo $?; }; f"), "0");
}
#[test]
fn assign_prefix_to_command_keeps_command_status() {
    assert_eq!(huck_stdout("x=$(exit 3) true; echo $?"), "0");
}
#[test]
fn append_assign_cmdsub_status() {
    assert_eq!(huck_stdout("x=a; x+=$(exit 4); echo $?"), "4");
}
