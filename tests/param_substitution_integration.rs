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
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn subst_first_match() {
    let (out, _) = run("name=foobar\necho ${name/o/X}\nexit\n");
    assert!(out.lines().any(|l| l == "fXobar"), "stdout: {out}");
}

#[test]
fn subst_all_matches() {
    let (out, _) = run("name=foobar\necho ${name//o/X}\nexit\n");
    assert!(out.lines().any(|l| l == "fXXbar"), "stdout: {out}");
}

#[test]
fn subst_missing_replacement_removes() {
    let (out, _) = run("name=foobar\necho ${name/o}\nexit\n");
    assert!(out.lines().any(|l| l == "fobar"), "stdout: {out}");
}

#[test]
fn subst_all_with_empty_replacement_removes_all() {
    let (out, _) = run("name=aaa\necho \"[${name//a}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn subst_anchored_prefix_hit() {
    let (out, _) = run("name=hello\necho ${name/#he/HI}\nexit\n");
    assert!(out.lines().any(|l| l == "HIllo"), "stdout: {out}");
}

#[test]
fn subst_anchored_prefix_miss_leaves_value() {
    let (out, _) = run("name=hello\necho ${name/#xo/HI}\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out}");
}

#[test]
fn subst_anchored_suffix_hit() {
    let (out, _) = run("name=hello\necho ${name/%lo/LO}\nexit\n");
    assert!(out.lines().any(|l| l == "helLO"), "stdout: {out}");
}

#[test]
fn subst_escaped_slash_in_pattern() {
    let (out, _) = run("path=a/b/c\necho ${path//\\//-}\nexit\n");
    assert!(out.lines().any(|l| l == "a-b-c"), "stdout: {out}");
}

#[test]
fn subst_inside_double_quotes_single_field() {
    // Pre-existing huck limitation: unquoted whitespace in ${...} operands
    // gets word-split (affects all modifiers, not just substitution). Use a
    // quoted-space pattern, which matches realistic bash usage.
    let (out, _) = run("name=\"foo bar\"\necho \"[${name/\" \"/_}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[foo_bar]"), "stdout: {out}");
}

#[test]
fn subst_glob_star_replaces_once() {
    let (out, _) = run("name=xyz\necho ${name//*/Q}\nexit\n");
    assert!(out.lines().any(|l| l == "Q"), "stdout: {out}");
}

#[test]
fn subst_unset_var_is_empty() {
    let (out, _) = run("echo \"[${MISSING/foo/bar}]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[]"), "stdout: {out}");
}

#[test]
fn subst_pattern_expansion_uses_other_var() {
    let (out, _) = run("name=foobar\np=o\necho ${name//$p/X}\nexit\n");
    assert!(out.lines().any(|l| l == "fXXbar"), "stdout: {out}");
}

#[test]
fn subst_replacement_expansion_uses_other_var() {
    let (out, _) = run("name=foobar\nr=Z\necho ${name//o/$r}\nexit\n");
    assert!(out.lines().any(|l| l == "fZZbar"), "stdout: {out}");
}

#[test]
fn subst_braced_var_in_pattern() {
    // ${var/${X}/Y} — the inner `}` must not prematurely close the outer
    // substitution; the lexer's depth-aware split picks the right `/`.
    let (out, _) = run("path=/home/user/bin\nh=/home/user\necho ${path/${h}/HOME}\nexit\n");
    assert!(out.lines().any(|l| l == "HOME/bin"), "stdout: {out}");
}

#[test]
fn subst_braced_var_in_replacement() {
    let (out, _) = run("name=foobar\nr=Z\necho ${name//o/${r}}\nexit\n");
    assert!(out.lines().any(|l| l == "fZZbar"), "stdout: {out}");
}

#[test]
fn subst_unicode_safe() {
    let (out, _) = run("name=café\necho ${name/é/E}\nexit\n");
    assert!(out.lines().any(|l| l == "cafE"), "stdout: {out}");
}

#[test]
fn subst_in_pipeline_stage() {
    let (out, _) = run("name=foo.txt\necho ${name/.txt/.md} | cat\nexit\n");
    assert!(out.lines().any(|l| l == "foo.md"), "stdout: {out}");
}
