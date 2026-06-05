//! v98: `&` async list separator (with and-or grouping).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn amp_separator_both_run() {
    // a & b : both run; serialize via wait + a marker so output is deterministic.
    assert_eq!(
        run("F=/tmp/huck_al1; : > $F\necho a >> $F &\nwait\necho b >> $F\nsort $F\n").0,
        "a\nb\n"
    );
}

#[test]
fn amp_in_for_body() {
    assert_eq!(
        run("F=/tmp/huck_al2; : > $F\nfor i in 1 2 3; do echo $i >> $F & done\nwait\nsort $F\n").0,
        "1\n2\n3\n"
    );
}

#[test]
fn amp_group_backgrounded_true_branch() {
    // `true && echo grouped &` : the group is backgrounded; grouped prints.
    assert_eq!(
        run("F=/tmp/huck_al3; : > $F\ntrue && echo grouped >> $F &\nwait\ncat $F\n").0,
        "grouped\n"
    );
}

#[test]
fn amp_group_backgrounded_false_shortcircuit() {
    // `false && echo no &` : group backgrounded, && short-circuits, `no` does NOT print.
    assert_eq!(
        run("F=/tmp/huck_al4; : > $F\nfalse && echo no >> $F &\nwait\ncat $F\n").0,
        ""
    );
}

#[test]
fn subshell_amp_backgrounds_left() {
    // ( a & b ): previously huck ran `a` foreground; now `a` is backgrounded. Both print.
    assert_eq!(
        run("F=/tmp/huck_al5; : > $F\n( echo a >> $F & wait; echo b >> $F )\nsort $F\n").0,
        "a\nb\n"
    );
}

#[test]
fn bang_pid_set() {
    let (out, _rc) = run("sleep 0 &\ncase \"$!\" in [0-9]*) echo numeric;; *) echo bad;; esac\n");
    assert_eq!(out, "numeric\n");
}

#[test]
fn trailing_amp_status_zero() {
    assert_eq!(run("false &\necho $?\nwait\n").0, "0\n");
}

#[test]
fn regression_semi_and_or_unchanged() {
    assert_eq!(
        run("true && echo y\nfalse || echo n\necho a; echo b\n").0,
        "y\nn\na\nb\n"
    );
}
