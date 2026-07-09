use std::process::Command;
fn huck(script: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", script])
        .output()
        .expect("run huck");
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test]
fn consecutive_lines() {
    assert_eq!(
        huck("echo $LINENO\necho $LINENO\necho $LINENO"),
        "1\n2\n3\n"
    );
}
#[test]
fn inside_function_reports_def_site_line() {
    assert_eq!(huck("f(){\n  echo $LINENO\n}\nf"), "2\n");
}
#[test]
fn if_condition_and_body_same_line() {
    assert_eq!(huck("if true; then echo $LINENO; fi"), "1\n");
}
#[test]
fn after_function_returns_resets() {
    assert_eq!(huck("f(){ :; }\nf\necho $LINENO"), "3\n");
}
#[test]
fn pipeline_stage_line() {
    // each stage is on its own line; the producing stage that reads $LINENO reports its line
    // bash -c $'echo $LINENO |\ncat' → 1
    assert_eq!(huck("echo $LINENO |\ncat"), "1\n");
}
#[test]
fn while_body_line() {
    // bash -c $'i=0\nwhile [ $i -lt 1 ]; do echo $LINENO; i=1; done' → 2
    assert_eq!(
        huck("i=0\nwhile [ $i -lt 1 ]; do echo $LINENO; i=1; done"),
        "2\n"
    );
}
#[test]
fn assignment_rhs_line() {
    // x=$LINENO on line 2 -> 2
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", "echo a\nx=$LINENO\necho $x"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a\n2\n");
}
#[test]
fn alias_expanded_command_reports_use_site_line() {
    // v237: a token from an alias body inherits the alias-name token's span, so
    // an alias whose body reads $LINENO reports the line where the alias is USED,
    // not where it was defined. `e` is used on lines 3 and 4 -> 3, 4 (matches bash).
    assert_eq!(
        huck("shopt -s expand_aliases\nalias e='echo $LINENO'\ne\ne"),
        "3\n4\n"
    );
}
