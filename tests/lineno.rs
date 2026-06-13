use std::process::Command;
fn huck(script: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", script]).output().expect("run huck");
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test] fn consecutive_lines() {
    assert_eq!(huck("echo $LINENO\necho $LINENO\necho $LINENO"), "1\n2\n3\n");
}
#[test] fn inside_function_reports_def_site_line() {
    assert_eq!(huck("f(){\n  echo $LINENO\n}\nf"), "2\n");
}
#[test] fn if_condition_and_body_same_line() {
    assert_eq!(huck("if true; then echo $LINENO; fi"), "1\n");
}
#[test] fn after_function_returns_resets() {
    assert_eq!(huck("f(){ :; }\nf\necho $LINENO"), "3\n");
}
#[test] fn pipeline_stage_line() {
    // each stage is on its own line; the producing stage that reads $LINENO reports its line
    // bash -c $'echo $LINENO |\ncat' → 1
    assert_eq!(huck("echo $LINENO |\ncat"), "1\n");
}
#[test] fn while_body_line() {
    // bash -c $'i=0\nwhile [ $i -lt 1 ]; do echo $LINENO; i=1; done' → 2
    assert_eq!(huck("i=0\nwhile [ $i -lt 1 ]; do echo $LINENO; i=1; done"), "2\n");
}
