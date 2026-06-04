//! Integration tests for v86 `shopt` builtin (M-08d).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Runs `script` through huck on stdin; returns (stdout, exit_code).
fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn shopt_oq_posix_returns_one_silently() {
    // The stock-bashrc case: `if ! shopt -oq posix`.
    let (out, rc) = run("shopt -oq posix; echo rc=$?\n");
    assert_eq!(out, "rc=1\n");
    assert_eq!(rc, 0);
}

#[test]
fn shopt_set_query_roundtrip() {
    assert_eq!(run("shopt -q nullglob; echo $?\n").0, "1\n");
    assert_eq!(run("shopt -s nullglob; shopt -q nullglob; echo $?\n").0, "0\n");
}

#[test]
fn shopt_inert_option_tracks() {
    // extglob is inert in huck but must round-trip.
    assert_eq!(run("shopt -s extglob; shopt -q extglob; echo $?\n").0, "0\n");
}

#[test]
fn shopt_invalid_name_rc_one() {
    let (_, rc) = run("shopt -s definitely_not_an_option\n");
    assert_eq!(rc, 1);
}

#[test]
fn shopt_query_prints_state() {
    assert_eq!(run("shopt -s dotglob; shopt dotglob\n").0, "dotglob        \ton\n");
}

#[test]
fn shopt_multi_query_rc_is_all_set() {
    // one on, one off → rc 1; both printed in table order.
    let (out, _) = run("shopt -s dotglob; shopt dotglob nullglob; echo rc=$?\n");
    assert_eq!(out, "dotglob        \ton\nnullglob       \toff\nrc=1\n");
}

#[test]
fn shopt_p_with_name_prints_reinput() {
    assert_eq!(run("shopt -p nullglob\n").0, "shopt -u nullglob\n");
    assert_eq!(run("shopt -s nullglob; shopt -p nullglob\n").0, "shopt -s nullglob\n");
}

#[test]
fn shopt_po_with_name_prints_reinput() {
    assert_eq!(run("shopt -po errexit\n").0, "set +o errexit\n");
}

#[test]
fn shopt_q_no_names_rc_zero() {
    assert_eq!(run("shopt -q; echo rc=$?\n").0, "rc=0\n");
    assert_eq!(run("shopt -oq; echo rc=$?\n").0, "rc=0\n");
}
