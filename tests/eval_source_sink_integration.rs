//! v132: eval/source run with the enclosing StdoutSink (capture/redirect),
//! not a fresh Terminal sink.
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
fn eval_captured_in_subst() {
    let (o, _e, _c) = run("x=$(eval 'echo hi'); echo \"[$x]\"\n");
    assert_eq!(o, "[hi]\n", "o: {o:?}");
}
#[test]
fn eval_multi_command_captured() {
    let (o, _e, _c) = run("x=$(eval 'echo a; echo b'); echo \"[$x]\"\n");
    assert_eq!(o, "[a\nb]\n", "o: {o:?}");
}
#[test]
fn eval_pipe_inside_capture() {
    let (o, _e, _c) = run("x=$(eval 'seq 1 100 | wc -l'); echo \"[$x]\"\n");
    assert_eq!(o.trim(), "[100]", "o: {o:?}");
}
#[test]
fn source_captured_in_subst() {
    let (o, _e, _c) = run("printf 'echo S\\n' > /tmp/v132src.sh\nx=$(source /tmp/v132src.sh); echo \"[$x]\"\n");
    assert_eq!(o, "[S]\n", "o: {o:?}");
}
#[test]
fn eval_top_level_prints() {
    let (o, _e, _c) = run("eval 'echo top'\n");
    assert_eq!(o, "top\n", "o: {o:?}");
}
#[test]
fn command_eval_captured() {
    let (o, _e, _c) = run("x=$(command eval 'echo c'); echo \"[$x]\"\n");
    assert_eq!(o, "[c]\n", "o: {o:?}");
}
#[test]
fn function_named_eval_shadows() {
    let (o, _e, _c) = run("eval() { echo fn; }\neval x\n");
    assert_eq!(o, "fn\n", "o: {o:?}");
}
#[test]
fn eval_redirect_to_file() {
    let (_o, _e, _c) = run("eval 'echo R' > /tmp/v132r.txt\n");
    let got = std::fs::read_to_string("/tmp/v132r.txt").unwrap_or_default();
    let _ = std::fs::remove_file("/tmp/v132r.txt");
    assert_eq!(got, "R\n", "file: {got:?}");
}
