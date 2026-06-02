use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn default_ifs_for_loop_splits_on_whitespace() {
    let (out, _, _) = run_capture("v=\"a  b\tc\"\nfor x in $v; do echo $x; done\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let words: Vec<&str> = lines.iter().filter(|l| ["a","b","c"].contains(l)).copied().collect();
    assert_eq!(words, vec!["a", "b", "c"], "got: {out:?}");
}

#[test]
fn colon_ifs_for_loop_splits_on_colons() {
    let (out, _, _) = run_capture("IFS=:\nv=\"a:b:c\"\nfor x in $v; do echo $x; done\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let words: Vec<&str> = lines.iter().filter(|l| ["a","b","c"].contains(l)).copied().collect();
    assert_eq!(words, vec!["a", "b", "c"], "got: {out:?}");
}

#[test]
fn colon_ifs_preserves_empty_middle_field() {
    let (out, _, _) = run_capture(
        "IFS=:\nv=\"a::b\"\nfor x in $v; do echo \"[$x]\"; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(lines, vec!["[a]", "[]", "[b]"], "got: {out:?}");
}

#[test]
fn colon_ifs_trailing_no_empty_field() {
    let (out, _, _) = run_capture(
        "IFS=:\nv=\"a:\"\nfor x in $v; do echo \"[$x]\"; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(lines, vec!["[a]"], "got: {out:?}");
}

#[test]
fn empty_ifs_no_splitting() {
    let (out, _, _) = run_capture(
        "IFS=\nv=\"a b c\"\nfor x in $v; do echo \"[$x]\"; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(lines, vec!["[a b c]"], "got: {out:?}");
}

#[test]
fn local_ifs_reverts_on_function_return() {
    let (out, _, _) = run_capture(
        "v=\"a:b\"\n\
         f() { local IFS=:; for x in $v; do echo \"in:$x\"; done; }\n\
         f\n\
         for x in $v; do echo \"out:$x\"; done\n\
         exit\n"
    );
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"in:a"), "got: {out:?}");
    assert!(lines.contains(&"in:b"), "got: {out:?}");
    assert!(lines.contains(&"out:a:b"), "got: {out:?}");
}

#[test]
fn command_sub_splits_with_current_ifs() {
    let (out, _, _) = run_capture(
        "IFS=:\nfor x in $(echo \"a:b:c\"); do echo $x; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().collect();
    let words: Vec<&str> = lines.iter().filter(|l| ["a","b","c"].contains(l)).copied().collect();
    assert_eq!(words, vec!["a", "b", "c"], "got: {out:?}");
}

#[test]
fn star_join_uses_first_ifs_char() {
    let (out, _, _) = run_capture(
        "set -- a b c\nIFS=,\necho \"$*\"\nexit\n"
    );
    assert!(out.lines().any(|l| l == "a,b,c"), "got: {out:?}");
}

#[test]
fn star_join_empty_ifs_concatenates() {
    let (out, _, _) = run_capture(
        "set -- a b c\nIFS=\necho \"$*\"\nexit\n"
    );
    assert!(out.lines().any(|l| l == "abc"), "got: {out:?}");
}

#[test]
fn inline_prefix_ifs_does_not_apply_to_this_commands_expansion() {
    // POSIX 2.9.1 + bash: inline-prefix assignments are saved and applied,
    // but word expansion of THIS command's arguments has already been
    // performed against the pre-prefix environment. So `IFS=: echo $v`
    // splits $v using the *outer* IFS (default whitespace), not `:`.
    // Bash 5.2.21 confirmed: outputs "a:b:c", not "a b c".
    let (out, _, _) = run_capture(
        "v=\"a:b:c\"\n\
         IFS=: echo $v\n\
         exit\n"
    );
    assert!(out.lines().any(|l| l == "a:b:c"), "got: {out:?}");
}
