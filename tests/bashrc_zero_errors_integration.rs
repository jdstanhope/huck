//! v109: bashrc-zero-errors integration tests.
//! M-90: builtin error output honors 2> / 2>> / 2>&1 redirection.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Returns (stdout, stderr, exit_code).
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
fn builtin_stderr_2_devnull_suppressed() {
    let (out, err, _c) = run("declare -p NOPE_VAR 2>/dev/null\necho after\n");
    assert_eq!(out, "after\n");
    assert!(!err.contains("NOPE_VAR"), "stderr leaked: {err}");
}

#[test]
fn builtin_stderr_unredirected_still_reaches_stderr() {
    let (_o, err, _c) = run("declare -p NOPE3\n");
    assert!(
        err.contains("NOPE3"),
        "stderr should still appear unredirected: {err}"
    );
}

#[test]
fn builtin_stderr_2_file_captured() {
    let (out, _e, _c) = run(
        "declare -p NOPE2 2>/tmp/huck_m90.err\necho \"got=[$(cat /tmp/huck_m90.err)]\"\nrm -f /tmp/huck_m90.err\n",
    );
    assert!(out.contains("NOPE2"), "stderr not captured to file: {out}");
}

#[test]
fn builtin_stderr_2_redirect_to_stdout_fd() {
    // `declare -p NOPE 2>&1 | grep` should let grep see the error text.
    let (out, _e, _c) = run("declare -p NOPE4 2>&1 | grep NOPE4 >/dev/null && echo matched\n");
    assert!(
        out.contains("matched"),
        "2>&1 did not route stderr to fd 1: {out}"
    );
}

// M-89: export accepts leading flags (-a no-op), fixing `mise activate bash`.
#[test]
fn export_a_flag_bare_no_output() {
    let (out, err, c) = run("export -a\necho done\n");
    assert_eq!(out, "done\n", "stdout should only contain 'done': {out}");
    assert_eq!(c, 0, "exit code should be 0");
    assert_eq!(err, "", "stderr should be empty: {err}");
}

#[test]
fn export_a_bare_name_exported() {
    let (out, err, _c) = run("export -a chpwd_functions\necho rc=$?\n");
    assert!(out.contains("rc=0"), "rc should be 0: {out}");
    assert_eq!(
        err, "",
        "stderr should be empty (no 'not a valid identifier'): {err}"
    );
}

#[test]
fn export_a_with_assignment_exports() {
    let (out, _e, c) = run("export -a FOO=bar\ndeclare -p FOO\n");
    assert!(
        out.contains("declare -x FOO=\"bar\""),
        "FOO should be exported: {out}"
    );
    assert_eq!(c, 0, "exit code should be 0");
}

#[test]
fn export_plain_assignment_unchanged() {
    let (out, _e, _c) = run("export NAME=v\ndeclare -p NAME\n");
    assert!(
        out.contains("declare -x NAME=\"v\""),
        "NAME should be exported: {out}"
    );
}

// M-82: ${arr[@]+word} / ${arr[@]-word} (and :+/:-, [*], assoc) on whole arrays.
#[test]
fn array_alt_set() {
    let (out, err, _c) = run("a=(x y z)\nprintf '[%s]' \"${a[@]+SET}\"\n");
    assert_eq!(out, "[SET]", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn array_alt_empty_is_unset() {
    let (out, err, _c) = run("b=()\nprintf '[%s]' \"${b[@]+SET}\"\n");
    assert_eq!(out, "[]", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn array_alt_unset() {
    let (out, err, _c) = run("unset c\nprintf '[%s]' \"${c[@]+SET}\"\n");
    assert_eq!(out, "[]", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn array_default_set_yields_elements() {
    let (out, err, _c) = run("a=(x y z)\nprintf '<%s>' \"${a[@]-DEF}\"\n");
    assert_eq!(out, "<x><y><z>", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn array_default_unset_yields_word() {
    let (out, err, _c) = run("b=()\nprintf '<%s>' \"${b[@]-DEF}\"\n");
    assert_eq!(out, "<DEF>", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn array_star_alt() {
    let (out, err, _c) = run("a=(x y z)\nprintf '[%s]' \"${a[*]+SET}\"\n");
    assert_eq!(out, "[SET]", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn array_safe_idiom() {
    let (out, err, _c) = run("set -u\na=(1 2)\nprintf '<%s>' \"${a[@]+\"${a[@]}\"}\"\n");
    assert_eq!(out, "<1><2>", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn array_idiom_unset_empty() {
    let (out, err, _c) = run("set -u\nunset c\nprintf '<%s>' \"${c[@]+\"${c[@]}\"}\"\necho END\n");
    assert!(out.contains("END"), "should print END: {out}");
    // The <...> part is empty: printf with zero args still emits one <>.
    assert_eq!(out, "<>END\n", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn assoc_alt_set() {
    let (out, err, _c) = run("declare -A m=([k]=v)\nprintf '[%s]' \"${m[@]+SET}\"\n");
    assert_eq!(out, "[SET]", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn assoc_alt_empty() {
    let (out, err, _c) = run("declare -A n=()\nprintf '[%s]' \"${n[@]+SET}\"\n");
    assert_eq!(out, "[]", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn assoc_default_unset() {
    let (out, err, _c) = run("declare -A n=()\nprintf '[%s]' \"${n[@]-DEF}\"\n");
    assert_eq!(out, "[DEF]", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}

#[test]
fn assoc_default_set_yields_values() {
    // Set assoc + `-DEF`: yields the array's values, not DEF. Single key
    // keeps iteration order deterministic.
    let (out, err, _c) = run("declare -A m=([k]=v)\nprintf '<%s>' \"${m[@]-DEF}\"\n");
    assert_eq!(out, "<v>", "out: {out}");
    assert_eq!(err, "", "stderr leaked: {err}");
}
