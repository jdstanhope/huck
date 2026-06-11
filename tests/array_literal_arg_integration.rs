//! v136: array-literal / assign-prefix word as a command ARGUMENT reconstructs
//! to text instead of panicking (M-114).
use std::process::{Command, Stdio};
use std::io::Write;
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut c = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn");
    c.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let o = c.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into_owned(),
     String::from_utf8_lossy(&o.stderr).into_owned(),
     o.status.code().unwrap_or(-1))
}
#[test]
fn eval_array_literal_assignment() {
    let (o, _e, _c) = run("eval x=(a b); echo \"${x[@]}|${#x[@]}\"\n");
    assert_eq!(o, "a b|2\n", "o: {o:?}");
}
#[test]
fn eval_array_append() {
    let (o, _e, _c) = run("arr=(p); eval arr+=(x y); echo \"${arr[@]}\"\n");
    assert_eq!(o, "p x y\n", "o: {o:?}");
}
#[test]
fn eval_indexed_element() {
    let (o, _e, _c) = run("eval a[1]=v; echo \"${a[1]}\"\n");
    assert_eq!(o, "v\n", "o: {o:?}");
}
#[test]
fn eval_indexed_append() {
    let (o, _e, _c) = run("a[2]=Q; eval a[2]+=z; echo \"${a[2]}\"\n");
    assert_eq!(o, "Qz\n", "o: {o:?}");
}
#[test]
fn eval_subscript_and_quoted_value() {
    // Through `eval`, the word `[3]="a b"` is expanded once (quotes removed) to
    // `[3]=a b`, then eval re-parses & word-splits -> [3]=a [4]=b [5]=c (bash).
    let (o, _e, _c) = run("eval x=([3]=\"a b\" c); echo \"${x[3]}|${x[4]}\"\n");
    assert_eq!(o, "a|b\n", "o: {o:?}");
}
#[test]
fn eval_empty_array() {
    let (o, _e, _c) = run("eval x=(); echo \"len=${#x[@]}\"\n");
    assert_eq!(o, "len=0\n", "o: {o:?}");
}
#[test]
fn escaped_form_still_works() {
    let (o, _e, _c) = run("f(){ eval $1=\\(p q\\); }; f arr; echo \"${arr[@]}\"\n");
    assert_eq!(o, "p q\n", "o: {o:?}");
}
#[test]
fn quoted_form_still_works() {
    let (o, _e, _c) = run("eval \"x=(a b)\"; echo \"${x[@]}\"\n");
    assert_eq!(o, "a b\n", "o: {o:?}");
}
#[test]
fn declaration_array_unchanged() {
    let (o, _e, _c) = run("declare d=(a b); echo \"${d[@]}\"\n");
    assert_eq!(o, "a b\n", "o: {o:?}");
}
#[test]
fn non_eval_arg_does_not_panic() {
    let (o, _e, c) = run("echo x=(a b)\n");
    assert_ne!(c, 101, "must not panic (rc 101)");
    assert_eq!(o, "x=(a b)\n", "o: {o:?}");
}
