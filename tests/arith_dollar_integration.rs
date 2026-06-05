//! v93: $-forms inside arithmetic contexts (M-88).
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
fn dollar_hash_in_dbl_paren() {
    assert_eq!(run("set -- a b\n(($# == 2)) && echo Y || echo N\n").0, "Y\n");
}

#[test]
fn arr_len_in_dbl_paren() {
    assert_eq!(run("a=(x y z)\n((${#a[@]} == 3)) && echo Y || echo N\n").0, "Y\n");
}

#[test]
fn param_expansion_in_arith_expansion() {
    assert_eq!(run("set -- -a5\necho $((${1#-a} + 2))\n").0, "7\n");
}

#[test]
fn command_sub_in_arith_expansion() {
    assert_eq!(run("echo $(( $(echo 3) * 4 ))\n").0, "12\n");
}

#[test]
fn dollar_in_arith_for_header() {
    assert_eq!(run("a=(x y z)\nfor ((i=0; i<${#a[@]}; i++)); do printf '%s' \"$i\"; done\necho\n").0, "012\n");
}

#[test]
fn bare_identifier_still_works() {
    assert_eq!(run("n=5\necho $((n + 1))\n").0, "6\n");
}

#[test]
fn quote_removal_in_arith() {
    assert_eq!(run("x=5\n(( x == \"5\" )) && echo Y || echo N\n").0, "Y\n");
}

#[test]
fn empty_arith_expansion_is_zero() {
    assert_eq!(run("e=\necho $(( e ))\n").0, "0\n");
}
