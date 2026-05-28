use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn ansi_c_tab_in_echo() {
    // $'a\tb' should print "a<TAB>b\n"
    let (out, _) = run("echo $'a\\tb'\nexit\n");
    assert!(out.lines().any(|l| l == "a\tb"), "stdout: {:?}", out);
}

#[test]
fn ansi_c_unicode_letter() {
    // $'é' should print "é\n"
    let (out, _) = run("echo $'\\u00e9'\nexit\n");
    assert!(out.lines().any(|l| l == "é"), "stdout: {:?}", out);
}

#[test]
fn ansi_c_hex_escapes_form_string() {
    // printf '%s' $'\x48\x69' → "Hi" (no trailing newline)
    let (out, _) = run("printf '%s' $'\\x48\\x69'\nexit\n");
    assert_eq!(out, "Hi", "stdout: {:?}", out);
}

#[test]
fn ansi_c_in_assignment_then_double_quoted_expansion() {
    // x=$'\n'; echo "[$x]" → "[<NL>]" — exact stdout is "[\n]\n"
    let (out, _) = run("x=$'\\n'\necho \"[$x]\"\nexit\n");
    assert_eq!(out, "[\n]\n", "stdout: {:?}", out);
}

#[test]
fn ansi_c_case_pattern_matches_decoded() {
    // case $'\t' in $'\t') echo yes ;; *) echo no ;; esac → "yes"
    let script = "case $'\\t' in\n  $'\\t') echo yes ;;\n  *) echo no ;;\nesac\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "yes"), "stdout: {:?}", out);
}

#[test]
fn ansi_c_concatenation_with_unquoted_suffix() {
    // echo $'a\tb'cd → "a<TAB>bcd"
    let (out, _) = run("echo $'a\\tb'cd\nexit\n");
    assert!(out.lines().any(|l| l == "a\tbcd"), "stdout: {:?}", out);
}
