// End-to-end driven through the binary so the full lex->expand->exec path is exercised.
use std::process::Command;
fn huck(script: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", script]).output().expect("run huck");
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test]
#[ignore = "payoff: enabled in Task 4 once expansion+exec are wired"]
fn cat_input_process_sub() {
    assert_eq!(huck("cat <(echo hi)"), "hi\n");
}
