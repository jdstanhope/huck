use std::process::Command;

fn huck(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck")).args(["-c", s]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn c_function_arrays() {
    // Verified vs: bash -c 'f(){ echo "[${BASH_SOURCE[@]}] [${BASH_LINENO[@]}] [${FUNCNAME[@]}]"; }; f'
    // bash output: "[environment] [1] [f]\n"
    assert_eq!(
        huck("f(){ echo \"[${BASH_SOURCE[@]}] [${BASH_LINENO[@]}] [${FUNCNAME[@]}]\"; }; f"),
        "[environment] [1] [f]\n"
    );
}

#[test]
fn c_top_level_unset() {
    // Verified vs: bash -c 'echo "[${BASH_SOURCE[@]:-x}] [${BASH_LINENO[@]:-x}] [${FUNCNAME[@]:-x}]"'
    // bash output: "[x] [x] [x]\n"
    assert_eq!(
        huck("echo \"[${BASH_SOURCE[@]:-x}] [${BASH_LINENO[@]:-x}] [${FUNCNAME[@]:-x}]\""),
        "[x] [x] [x]\n"
    );
}

#[test]
fn c_nested_functions() {
    // Verified vs: bash -c $'inner(){ echo "[${BASH_SOURCE[@]}] [${FUNCNAME[@]}]"; }\nouter(){ inner; }\nouter'
    // bash output: "[environment environment] [inner outer]\n"
    let got = huck("inner(){ echo \"[${BASH_SOURCE[@]}] [${FUNCNAME[@]}]\"; }\nouter(){ inner; }\nouter");
    assert_eq!(got, "[environment environment] [inner outer]\n");
}
