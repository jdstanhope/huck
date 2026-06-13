use std::process::Command;

fn huck(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck")).args(["-c", s]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Run huck with a two-file setup: a lib file and a main file that sources it.
/// `main_body` contains `%LIB%` which is replaced with the absolute path to the lib file.
fn huck_two_file(main_body: &str, lib_body: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std::process::id();
    let lib = std::env::temp_dir().join(format!("huck_v153_lib_{pid}_{nanos}.sh"));
    let mainf = std::env::temp_dir().join(format!("huck_v153_main_{pid}_{nanos}.sh"));
    std::fs::write(&lib, lib_body).unwrap();
    let body = main_body.replace("%LIB%", lib.to_str().unwrap());
    std::fs::write(&mainf, &body).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg(&mainf)
        .output()
        .unwrap();
    std::fs::remove_file(&lib).ok();
    std::fs::remove_file(&mainf).ok();
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

#[test]
fn sourced_top_level_source_stack() {
    // Source a lib at top level (line 2 of main).
    // Verified vs bash: FUNCNAME is unset (no function frame), BASH_SOURCE has 2 elements
    // (lib path, main path), BASH_LINENO=[2 0] (source was on line 2 of main; main base is 0).
    //
    // bash "$main" where main is:
    //   echo top
    //   source $lib    <- line 2
    // and lib is:
    //   echo "FN=[${FUNCNAME[@]:-unset}] SRC_COUNT=${#BASH_SOURCE[@]} SRC0=${BASH_SOURCE[0]} SRC1=${BASH_SOURCE[1]} LN=[${BASH_LINENO[@]}]"
    //
    // Bash output (basenames vary):
    //   top
    //   FN=[unset] SRC_COUNT=2 SRC0=<lib_path> SRC1=<main_path> LN=[2 0]
    //
    // Note: ##*/ on arrays not yet supported in huck; print individual elements instead.
    let lib = concat!(
        "echo \"FN=[${FUNCNAME[@]:-unset}]",
        " SRC_COUNT=${#BASH_SOURCE[@]}",
        " SRC0=${BASH_SOURCE[0]}",
        " SRC1=${BASH_SOURCE[1]}",
        " LN=[${BASH_LINENO[@]}]\"\n"
    );
    let main = "echo top\nsource %LIB%\n"; // source on line 2 of main
    let got = huck_two_file(main, lib);
    // Line 1: "top\n"
    assert!(got.starts_with("top\n"), "missing top line; got: {got}");
    // FUNCNAME is unset (no function frame active during top-level source)
    assert!(got.contains("FN=[unset]"), "FUNCNAME should be unset at top-level source; got: {got}");
    // BASH_SOURCE has 2 elements: lib path and main path
    assert!(got.contains("SRC_COUNT=2"), "BASH_SOURCE should have 2 elements; got: {got}");
    // SRC0 is the lib path (contains "huck_v153_lib_")
    assert!(got.contains("huck_v153_lib_"), "SRC0 should be the lib path; got: {got}");
    // SRC1 is the main path (contains "huck_v153_main_")
    assert!(got.contains("huck_v153_main_"), "SRC1 should be the main path; got: {got}");
    // BASH_LINENO: [2 0] — source was called on line 2 of main; main base is 0
    assert!(got.contains("LN=[2 0]"), "BASH_LINENO should be [2 0]; got: {got}");
}

#[test]
fn function_in_sourced_lib() {
    // A function defined in a sourced lib is called from a function in the main script.
    // Verified vs bash: FUNCNAME=[libfn caller main].
    // BASH_SOURCE has 3 elements: [lib path, main path, main path].
    //
    // bash "$main" where main is:
    //   source $lib     <- line 1
    //   caller(){ libfn; }
    //   caller
    // and lib is:
    //   libfn(){ echo "FN=[${FUNCNAME[@]}] SRC_COUNT=${#BASH_SOURCE[@]} SRC0=${BASH_SOURCE[0]} SRC1=${BASH_SOURCE[1]}"; }
    //
    // At the time libfn runs the source frame is gone; only function frames remain.
    // Bash output (basenames vary):
    //   FN=[libfn caller main] SRC_COUNT=3 SRC0=<lib_path> SRC1=<main_path>
    //
    // Note: ##*/ on arrays not yet supported in huck; print individual elements instead.
    let lib = concat!(
        "libfn(){",
        " echo \"FN=[${FUNCNAME[@]}]",
        " SRC_COUNT=${#BASH_SOURCE[@]}",
        " SRC0=${BASH_SOURCE[0]}",
        " SRC1=${BASH_SOURCE[1]}\";",
        " }\n"
    );
    let main = "source %LIB%\ncaller(){ libfn; }\ncaller\n";
    let got = huck_two_file(main, lib);
    // FUNCNAME: libfn (defined in lib), caller and main (defined in main)
    assert!(got.contains("FN=[libfn caller main]"), "FUNCNAME wrong; got: {got}");
    // BASH_SOURCE has 3 elements: lib (where libfn defined), main (where caller is), main (top)
    assert!(got.contains("SRC_COUNT=3"), "BASH_SOURCE should have 3 elements; got: {got}");
    // SRC0 is the lib path
    assert!(got.contains("huck_v153_lib_"), "SRC0 should be the lib path; got: {got}");
    // SRC1 is the main path
    assert!(got.contains("huck_v153_main_"), "SRC1 should be the main path; got: {got}");
}

#[test]
fn source_inside_function_shows_source_in_funcname() {
    // When source is called inside a function, bash shows "source" as the top FUNCNAME entry.
    // Verified vs bash: FUNCNAME=[source myfunc main], BASH_SOURCE has 3 elements,
    // BASH_LINENO=[2 4 0].
    //
    // bash "$main" where main is:
    //   myfunc() {      <- line 1
    //     source $lib   <- line 2
    //   }               <- line 3
    //   myfunc          <- line 4
    // and lib is:
    //   echo "FN=[${FUNCNAME[@]}] SRC_COUNT=${#BASH_SOURCE[@]} SRC0=${BASH_SOURCE[0]} LN=[${BASH_LINENO[@]}]"
    //
    // Bash output (basenames vary):
    //   FN=[source myfunc main] SRC_COUNT=3 SRC0=<lib_path> LN=[2 4 0]
    //
    // Note: ##*/ on arrays not yet supported in huck; print individual elements instead.
    let lib = concat!(
        "echo \"FN=[${FUNCNAME[@]}]",
        " SRC_COUNT=${#BASH_SOURCE[@]}",
        " SRC0=${BASH_SOURCE[0]}",
        " LN=[${BASH_LINENO[@]}]\"\n"
    );
    let main = "myfunc() {\n  source %LIB%\n}\nmyfunc\n"; // source on line 2 of myfunc body; myfunc called on line 4
    let got = huck_two_file(main, lib);
    // FUNCNAME includes "source" (the source builtin) as the innermost frame
    assert!(got.contains("FN=[source myfunc main]"), "FUNCNAME should show source frame; got: {got}");
    // BASH_SOURCE has 3 elements: lib, main, main
    assert!(got.contains("SRC_COUNT=3"), "BASH_SOURCE should have 3 elements; got: {got}");
    // SRC0 is the lib path
    assert!(got.contains("huck_v153_lib_"), "SRC0 should be the lib path; got: {got}");
    // BASH_LINENO: source on line 2 of myfunc body, myfunc called on line 4 of main, base is 0
    assert!(got.contains("LN=[2 4 0]"), "BASH_LINENO should be [2 4 0]; got: {got}");
}
