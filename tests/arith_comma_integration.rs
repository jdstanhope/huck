//! v112: arithmetic comma operator integration tests (M-108).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn double_paren_comma_sets_both() {
    let (out, _e, _c) = run("(( a=1, b=2 ))\necho \"$a $b\"\n");
    assert_eq!(out, "1 2\n", "out: {out}");
}

#[test]
fn dollar_arith_comma_value_is_last() {
    let (out, _e, _c) = run("echo $((1, 2, 3))\n");
    assert_eq!(out, "3\n", "out: {out}");
}

#[test]
fn comma_inside_parens_in_dollar_arith() {
    let (out, _e, _c) = run("echo $(( (1,2) + 3 ))\n");
    assert_eq!(out, "5\n", "out: {out}");
}

#[test]
fn comma_below_assignment() {
    let (out, _e, _c) = run("a=9\necho $(( a=1, 2 ))\necho \"$a\"\n");
    assert_eq!(out, "2\n1\n", "out: {out}");
}

#[test]
fn c_style_for_comma_in_init_and_update() {
    let (out, _e, _c) = run("for ((i=0,j=0; i<3; i++,j++)); do echo \"$i:$j\"; done\n");
    assert_eq!(out, "0:0\n1:1\n2:2\n", "out: {out}");
}

#[test]
fn reassemble_comp_words_shape() {
    // The bash_completion __reassemble loop shape that started this: a C-style
    // for with comma over a COMP_WORDS-like array must run without the
    // `((: unexpected character: ','` error.
    let (out, err, _c) = run(
        "COMP_WORDS=(mise \"\")\n\
         for ((i=0,j=0; i<${#COMP_WORDS[@]}; i++,j++)); do echo \"w$i=${COMP_WORDS[i]}\"; done\n\
         echo done\n");
    assert!(out.contains("done"), "loop did not complete: {out} / {err}");
    assert!(!err.contains("unexpected character"), "comma error leaked: {err}");
}
