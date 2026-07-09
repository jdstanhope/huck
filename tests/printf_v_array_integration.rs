//! v113: `printf -v` array-element target integration tests (M-109).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn printf_v_indexed_elements() {
    let (out, err, _c) = run(
        "words=()\nprintf -v \"words[0]\" %s a\nprintf -v \"words[1]\" %s b\n\
         echo \"${words[0]}/${words[1]}\"\n",
    );
    assert_eq!(out, "a/b\n", "out: {out} err: {err}");
    assert!(
        !err.contains("not a valid identifier"),
        "rejected target: {err}"
    );
}

#[test]
fn printf_v_arith_subscript() {
    let (out, _e, _c) = run("j=2\nprintf -v \"x[j+1]\" %s X\ndeclare -p x\n");
    assert!(out.contains("[3]=\"X\""), "out: {out}");
}

#[test]
fn printf_v_unset_var_promotes_to_indexed_array() {
    let (out, _e, _c) = run("printf -v \"y[2]\" %s hi\ndeclare -p y\n");
    assert_eq!(out, "declare -a y=([2]=\"hi\")\n", "out: {out}");
}

#[test]
fn printf_v_associative_element() {
    let (out, _e, _c) = run("declare -A m\nprintf -v \"m[key]\" %s V\necho \"${m[key]}\"\n");
    assert_eq!(out, "V\n", "out: {out}");
}

#[test]
fn printf_v_plain_name_unchanged() {
    let (out, _e, _c) = run("printf -v plain %s hello\necho \"$plain\"\n");
    assert_eq!(out, "hello\n", "out: {out}");
}

#[test]
fn printf_v_reassemble_loop_shape() {
    // The bash_completion __reassemble shape: build a `words` array element by
    // element with printf -v "words[i]". Must populate, no identifier error.
    let (out, err, _c) = run("COMP_WORDS=(mise \"\")\nwords=()\n\
         for ((i=0; i<${#COMP_WORDS[@]}; i++)); do printf -v \"words[i]\" %s \"${COMP_WORDS[i]}\"; done\n\
         echo \"n=${#words[@]} w0=${words[0]}\"\n");
    assert_eq!(out, "n=2 w0=mise\n", "out: {out} err: {err}");
    assert!(!err.contains("not a valid identifier"), "leak: {err}");
}
