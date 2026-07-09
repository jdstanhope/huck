//! v100: subshell/compound-headed pipeline in any sequence position (M-11a).
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
fn subshell_pipe_after_semi() {
    assert_eq!(run("echo z; ( echo a ) | sort\n").0, "z\na\n");
}

#[test]
fn subshell_pipe_after_and() {
    assert_eq!(run("true && ( printf 'b\\na\\n' ) | sort\n").0, "a\nb\n");
}

#[test]
fn subshell_pipe_after_or() {
    assert_eq!(run("false || ( echo x ) | cat\n").0, "x\n");
}

#[test]
fn brace_group_pipe_after_semi() {
    assert_eq!(run("echo z; { echo a; echo b; } | sort\n").0, "z\na\nb\n");
}

#[test]
fn if_pipe_after_semi() {
    assert_eq!(run("echo z; if true; then echo a; fi | cat\n").0, "z\na\n");
}

#[test]
fn subshell_pipe_in_function_body() {
    assert_eq!(run("f() { echo z; ( echo a ) | sort; }\nf\n").0, "z\na\n");
}

#[test]
fn subshell_pipe_in_for_body() {
    assert_eq!(
        run("for i in 1 2; do ( echo $i ) | cat; done\n").0,
        "1\n2\n"
    );
}

#[test]
fn nvm_shaped_function() {
    // local + ( for ... & done; wait ) | sort  inside a function (the nvm shape).
    assert_eq!(
        run("f() {\n  local X\n  ( for n in b a; do echo $n & done; wait ) | sort\n}\nf\n").0,
        "a\nb\n"
    );
}

#[test]
fn subshell_pipe_after_amp() {
    // `( ) | cmd` as an Amp-separated element (v98).
    assert_eq!(run("( echo a ) | cat & wait\necho done\n").0, "a\ndone\n");
}

#[test]
fn negated_subshell_pipe_after_semi() {
    // `! ( false ) | cat` negation through the helper.
    // bash: `cat` exits 0 -> pipeline 0 -> negated 1 -> rc=1.
    assert_eq!(
        run("echo z; ! ( false ) | cat; echo rc=$?\n").0,
        "z\nrc=1\n"
    );
}

#[test]
fn regression_plain_sequences_unchanged() {
    assert_eq!(
        run("echo a; echo b\ntrue && echo y\nfalse || echo n\necho p | cat\n").0,
        "a\nb\ny\nn\np\n"
    );
}

#[test]
fn regression_subshell_pipe_first_position() {
    assert_eq!(run("( echo a ) | sort; echo z\n").0, "a\nz\n");
}
