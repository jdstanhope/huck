// End-to-end FUNCNAME, driven through the binary.
use std::process::Command;
fn huck(script: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", script]).output().expect("run huck");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test] fn scalar_is_current_function() {
    assert_eq!(huck(r#"f(){ echo "$FUNCNAME"; }; f"#), "f\n");
}
#[test] fn array_is_inner_to_outer() {
    assert_eq!(huck(r#"inner(){ echo "${FUNCNAME[@]}"; }; outer(){ inner; }; outer"#), "inner outer\n");
}
#[test] fn depth_and_caller() {
    assert_eq!(huck(r#"inner(){ echo "${#FUNCNAME[@]} ${FUNCNAME[1]}"; }; outer(){ inner; }; outer"#), "2 outer\n");
}
#[test] fn restored_after_nested_return() {
    assert_eq!(huck(r#"g(){ echo "$FUNCNAME"; }; f(){ g; echo "$FUNCNAME"; }; f"#), "g\nf\n");
}
#[test] fn unset_at_top_level() {
    assert_eq!(huck(r#"echo "[${FUNCNAME:-unset}] ${#FUNCNAME[@]}"; f(){ :; }; f; echo "[${FUNCNAME:-unset}]""#), "[unset] 0\n[unset]\n");
}
