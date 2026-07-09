//! v139: HISTSIZE/HISTFILESIZE honored from the variable table (M-59). Deterministic
//! via piped stdin + a temp HISTFILE; the vars are set in the spawn env (huck
//! imports the env into its variable table at startup, so they're visible from the
//! first recorded command). Asserts exact histfile contents.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run huck with piped `script` on stdin and the given extra env vars; return the
/// resulting HISTFILE contents.
fn run_hist(envs: &[(&str, &str)], script: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("hist");
    let mut cmd = Command::new(huck_bin());
    cmd.env("HISTFILE", &hf);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().expect("spawn huck");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    child.wait().unwrap();
    std::fs::read_to_string(&hf).unwrap_or_default()
}

#[test]
fn histsize_caps_in_memory_list() {
    let out = run_hist(&[("HISTSIZE", "2")], "echo a\necho b\necho c\n");
    assert_eq!(out, "echo b\necho c\n", "out={out:?}");
}

#[test]
fn histfilesize_caps_file_below_histsize() {
    let out = run_hist(
        &[("HISTSIZE", "100"), ("HISTFILESIZE", "1")],
        "echo a\necho b\n",
    );
    assert_eq!(out, "echo b\n", "out={out:?}");
}

#[test]
fn histsize_negative_is_unlimited() {
    let out = run_hist(&[("HISTSIZE", "-1")], "echo a\necho b\necho c\necho d\n");
    assert_eq!(out, "echo a\necho b\necho c\necho d\n", "out={out:?}");
}

#[test]
fn histsize_zero_empties() {
    let out = run_hist(&[("HISTSIZE", "0")], "echo a\necho b\n");
    assert_eq!(out, "", "out={out:?}");
}

#[test]
fn histfilesize_zero_empties_file() {
    let out = run_hist(&[("HISTFILESIZE", "0")], "echo a\n");
    assert_eq!(out, "", "out={out:?}");
}

#[test]
fn default_unset_keeps_all_small() {
    let out = run_hist(&[], "echo a\necho b\n");
    assert_eq!(out, "echo a\necho b\n", "out={out:?}");
}
