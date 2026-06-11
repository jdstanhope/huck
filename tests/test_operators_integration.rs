//! v135: M-27 test/[[ file-type/mode/fd operators.
use std::process::{Command, Stdio};
use std::io::Write;
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, i32) {
    let mut c = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn");
    c.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
}

#[test]
fn char_special_dev_null() {
    assert_eq!(run("[ -c /dev/null ] && echo T || echo F\n").0, "T\n");
    assert_eq!(run("[[ -c /dev/null ]] && echo T || echo F\n").0, "T\n");
}
#[test]
fn char_special_regular_file_false() {
    assert_eq!(run("printf x > /tmp/v135reg; [ -c /tmp/v135reg ] && echo T || echo F; rm -f /tmp/v135reg\n").0, "F\n");
}
#[test]
fn fifo_via_mkfifo() {
    let s = "D=$(mktemp -d); mkfifo $D/f; [ -p $D/f ] && echo T || echo F; [ -p /dev/null ] && echo pT || echo pF; rm -rf $D\n";
    assert_eq!(run(s).0, "T\npF\n");
}
#[test]
fn block_special_dev_null_is_not_block() {
    assert_eq!(run("[ -b /dev/null ] && echo T || echo F\n").0, "F\n");
}
#[test]
fn owned_by_euid_true_for_own_file() {
    assert_eq!(run("f=$(mktemp); [ -O $f ] && echo T || echo F; rm -f $f\n").0, "T\n");
}
#[test]
fn sticky_setuid_setgid() {
    let s = "f=$(mktemp); chmod u+s,g+s $f; [ -u $f ] && echo uT || echo uF; [ -g $f ] && echo gT || echo gF; rm -f $f; \
             d=$(mktemp -d); chmod +t $d; [ -k $d ] && echo kT || echo kF; rm -rf $d\n";
    assert_eq!(run(s).0, "uT\ngT\nkT\n");
}
#[test]
fn terminal_fd_false_when_redirected() {
    assert_eq!(run("[ -t 0 ] </dev/null && echo T || echo F\n").0, "F\n");
    assert_eq!(run("[ -t 99 ] && echo T || echo F\n").0, "F\n");
    assert_eq!(run("[ -t abc ] && echo T || echo F\n").0, "F\n");
}
#[test]
fn missing_file_all_false() {
    for op in ["-p","-S","-b","-c","-O","-G","-k","-u","-g"] {
        let s = format!("[ {op} /no/such/path/v135 ] && echo T || echo F\n");
        assert_eq!(run(&s).0, "F\n", "op {op}");
    }
}
