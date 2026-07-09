//! v102: braced single-char special params ${-}/${?}/${$}/${!} (+ modifiers).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn braced_status() {
    assert_eq!(run("false; echo \"${?}\"\n").0, "1\n");
    assert_eq!(run("true; echo \"${?}\"\n").0, "0\n");
}

#[test]
fn braced_status_with_modifier() {
    // ${?:-x}: status is set (0), so :- yields the status, not the default.
    assert_eq!(run("true; echo \"${?:-na}\"\n").0, "0\n");
}

#[test]
fn braced_pid_equals_dollar_dollar() {
    assert_eq!(
        run("[ \"${$}\" = \"$$\" ] && echo same || echo diff\n").0,
        "same\n"
    );
}

#[test]
fn braced_dash_equals_unbraced() {
    assert_eq!(
        run("[ \"${-}\" = \"$-\" ] && echo same || echo diff\n").0,
        "same\n"
    );
}

#[test]
fn braced_dash_remove_prefix_nvm_shape() {
    // nvm's errexit test. Under default (no -e): ${-#*e} == $- -> "no".
    assert_eq!(
        run("f() { if [ \"${-#*e}\" != \"$-\" ]; then echo yes; else echo no; fi; }\nf\n").0,
        "no\n"
    );
    // With -e set: removing up-to-e changes it -> "yes".
    assert_eq!(
        run("set -e\nf() { if [ \"${-#*e}\" != \"$-\" ]; then echo yes; else echo no; fi; }\nf\n")
            .0,
        "yes\n"
    );
}

#[test]
fn braced_bgpid_empty_then_set() {
    assert_eq!(run("[ -z \"${!}\" ] && echo empty\n").0, "empty\n");
    assert_eq!(
        run("sleep 0 &\n[ -n \"${!}\" ] && echo set\nwait\n").0,
        "set\n"
    );
}

#[test]
fn regression_braced_count_allargs_indirect_unchanged() {
    assert_eq!(
        run("set -- a b c\necho \"${#}\"\necho \"${@}\"\nx=hi; r=x; echo \"${!r}\"\n").0,
        "3\na b c\nhi\n"
    );
}
