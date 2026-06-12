use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Resolved form of `p` — what huck's `cd`/`pushd` will end up using
/// after canonicalizing symlinks via `env::current_dir()`. On Linux
/// `/tmp` stays `/tmp`; on macOS it becomes `/private/tmp`.
fn canonical(p: &str) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|e| panic!("canonicalize {p}: {e}"))
        .to_string_lossy()
        .into_owned()
}

fn run_capture(script: &str) -> (String, String, i32) {
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn pushd_dir_then_dirs() {
    let (out, _, _) = run_capture("pushd /tmp\ndirs\nexit\n");
    // After pushd, dirs output starts with the resolved /tmp.
    let tmp = canonical("/tmp");
    assert!(
        out.lines().any(|l| l.starts_with(tmp.as_str())),
        "stdout: {out:?}",
    );
}

#[test]
fn pushd_then_popd_returns_to_origin() {
    let (out, _, _) = run_capture(
        "ORIG=$PWD\npushd /tmp\npopd\necho \"AT $PWD\"\necho \"WANT $ORIG\"\nexit\n",
    );
    let at = out.lines().find(|l| l.starts_with("AT ")).unwrap_or("?");
    let want = out.lines().find(|l| l.starts_with("WANT ")).unwrap_or("?");
    assert_eq!(
        at.strip_prefix("AT ").unwrap_or(""),
        want.strip_prefix("WANT ").unwrap_or("X"),
        "stdout: {out:?}",
    );
}

#[test]
fn pushd_no_args_swaps_top_two() {
    let (out, _, _) = run_capture(
        "pushd /tmp\npushd /var\npushd\necho \"AT $PWD\"\nexit\n",
    );
    let tmp = canonical("/tmp");
    let want = format!("AT {tmp}");
    assert!(
        out.lines().any(|l| l == want),
        "expected pwd == {tmp} after swap; stdout: {out:?}",
    );
}

#[test]
fn pushd_only_one_entry_errors() {
    let (_out, err, _) = run_capture("pushd\necho rc=$?\nexit\n");
    assert!(
        err.contains("no other directory"),
        "stderr: {err:?}",
    );
}

#[test]
fn popd_empty_errors() {
    let (_out, err, _) = run_capture("popd\necho rc=$?\nexit\n");
    assert!(
        err.contains("directory stack empty"),
        "stderr: {err:?}",
    );
}

#[test]
fn dirs_default_collapses_home() {
    let (out, _, _) = run_capture(
        "export HOME=$PWD\ndirs\nexit\n",
    );
    // After HOME=cwd, dirs default prints just `~`.
    assert!(
        out.lines().any(|l| l == "~"),
        "stdout: {out:?}",
    );
}

#[test]
fn dirs_v_numbered() {
    let (out, _, _) = run_capture(
        "pushd /tmp\npushd /var\ndirs -v\nexit\n",
    );
    // Expect 3 numbered lines: " 0", " 1", " 2".
    let numbered = out
        .lines()
        .filter(|l| l.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit()))
        .count();
    assert!(
        numbered >= 3,
        "expected at least 3 numbered lines; stdout: {out:?}",
    );
}

#[test]
fn dirs_c_clears() {
    let (out, _, _) = run_capture(
        "pushd /tmp\ndirs -c\ndirs\nexit\n",
    );
    // After -c, dirs should print just one entry (the current dir).
    // Find the last non-empty line printed by `dirs`.
    let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    let last = lines.last().copied().unwrap_or("");
    assert!(
        !last.contains(' '),
        "expected single entry (no space-join); last line: {last:?}",
    );
}

#[test]
fn pushd_plus_n_rotates() {
    // Stack: [<cwd>, /var, /tmp]  (after the two pushes, /var
    // is on top because pushd inserts to front).
    // Wait — pushd inserts the NEW dir at front. So after
    // `pushd /tmp; pushd /var`: stack is [/var, /tmp, <orig>].
    // `pushd +2` rotates so index 2 (orig cwd) is top.
    let (out, _, _) = run_capture(
        "ORIG=$PWD\npushd /tmp\npushd /var\npushd +2\necho \"AT $PWD\"\necho \"WANT $ORIG\"\nexit\n",
    );
    let at = out.lines().find(|l| l.starts_with("AT ")).unwrap_or("");
    let want = out.lines().find(|l| l.starts_with("WANT ")).unwrap_or("");
    assert_eq!(
        at.strip_prefix("AT ").unwrap_or("x"),
        want.strip_prefix("WANT ").unwrap_or("y"),
        "stdout: {out:?}",
    );
}

#[test]
fn dirs_plus_index_prints_one() {
    let (out, _, _) = run_capture(
        "pushd /tmp\ndirs +1\nexit\n",
    );
    // dirs +1 prints just the second entry (the original cwd,
    // which would have ~ collapse if it matches HOME, or its
    // absolute path otherwise).
    let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    // Find the line just before `exit` was processed — last non-blank
    // line preceding the final shell-exit cleanup output.
    assert!(
        !lines.is_empty(),
        "expected at least one output line; stdout: {out:?}",
    );
    // The dirs +1 output line shouldn't contain a space (single entry).
    let last_dirs = lines
        .iter()
        .rev()
        .find(|l| !l.contains(' '))
        .copied()
        .unwrap_or("");
    assert!(
        !last_dirs.is_empty(),
        "expected a single-entry dirs +1 line; stdout: {out:?}",
    );
}
