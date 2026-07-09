//! v114: alternate/default word quoting under unquoted outer expansion (M-110).
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
fn array_alt_empty_element_preserved() {
    let (out, _e, _c) =
        run("a=(x \"\" y)\nset -- ${a[@]+\"${a[@]}\"}\necho $#\nprintf '<%s>' \"$@\"\necho\n");
    assert_eq!(out, "3\n<x><><y>\n", "out: {out}");
}

#[test]
fn array_alt_spaced_element_not_resplit() {
    let (out, _e, _c) = run("a=(\"a b\" c)\nset -- ${a[@]+\"${a[@]}\"}\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn scalar_alt_quoted_inner_one_field() {
    let (out, _e, _c) = run("x=1\nset -- ${x+\"a b\"}\necho $#\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn scalar_alt_unquoted_inner_splits() {
    let (out, _e, _c) = run("x=1\nset -- ${x+a b}\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn fully_quoted_outer_unchanged_array() {
    let (out, _e, _c) = run("a=(x \"\" y)\nset -- \"${a[@]+\"${a[@]}\"}\"\necho $#\n");
    assert_eq!(out, "3\n", "out: {out}");
}

#[test]
fn fully_quoted_outer_unchanged_scalar() {
    let (out, _e, _c) = run("x=1\nset -- \"${x+a b}\"\necho $#\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn default_word_unset_unquoted_splits() {
    let (out, _e, _c) = run("unset u\nset -- ${u-a b}\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn assoc_alt_spaced_value_preserved() {
    let (out, _e, _c) = run("declare -A m=([k]=\"a b\")\nset -- ${m[@]+\"${m[@]}\"}\necho $#\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn upvars_mise_shape_arg_count() {
    // The exact bash_completion __get_cword_at_cursor_by_ref shape: the empty
    // trailing element must survive so the -a${#words[@]} count matches.
    let (out, _e, _c) = run(
        "words=(mise \"\")\nset -- -a${#words[@]} words ${words+\"${words[@]}\"} -v cword 1\necho $#\n",
    );
    assert_eq!(out, "7\n", "out: {out}");
}
