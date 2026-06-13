use std::process::Command;

fn huck(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck")).args(["-c", s]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn huck_file(body: &str) -> String {
    let f = std::env::temp_dir().join(format!(
        "huck_v153_{}_{}.sh",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&f, body).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_huck")).arg(&f).output().unwrap();
    std::fs::remove_file(&f).ok();
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

#[test]
fn script_in_function_has_main_frame() {
    // g defined L1, body echo on L1; f L2 calls g; f called L3.
    // Verified vs bash <file>: bash output: "[g f main] [2 3 0]\n"
    let body = "g(){ echo \"[${FUNCNAME[@]}] [${BASH_LINENO[@]}]\"; }\nf(){ g; }\nf\n";
    assert_eq!(huck_file(body), "[g f main] [2 3 0]\n");
}

#[test]
fn script_top_level_source_set_funcname_unset() {
    // BASH_SOURCE[0] is the script path (basename check); length 1; FUNCNAME unset; BASH_LINENO[0] is 0.
    // Verified vs bash <file>: bash output: "[<basename>.sh-suffix] [1] [unset] [0]\n"
    let body = "echo \"[${BASH_SOURCE[0]##*/}-suffix] [${#BASH_SOURCE[@]}] [${FUNCNAME[@]:-unset}] [${BASH_LINENO[@]}]\"\n";
    let out = huck_file(body);
    assert!(out.contains("-suffix] [1] [unset] [0]"), "got: {out}");
    assert!(out.contains(".sh-suffix"), "BASH_SOURCE[0] should be the script path, got: {out}");
}
