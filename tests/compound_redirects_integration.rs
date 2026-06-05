//! v97: redirections on compound commands.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn heredoc_on_while_done() {
    assert_eq!(run("while read x; do echo \"g:$x\"; done <<EOF\na\nb\nEOF\n").0, "g:a\ng:b\n");
}

#[test]
fn redirect_out_on_for() {
    assert_eq!(run("for i in 1 2; do echo $i; done > /tmp/huck_t_for\ncat /tmp/huck_t_for\n").0, "1\n2\n");
}

#[test]
fn redirect_out_on_if() {
    assert_eq!(run("if true; then echo hi; fi > /tmp/huck_t_if\ncat /tmp/huck_t_if\n").0, "hi\n");
}

#[test]
fn redirect_out_on_brace_group() {
    assert_eq!(run("{ echo a; echo b; } > /tmp/huck_t_bg\ncat /tmp/huck_t_bg\n").0, "a\nb\n");
}

#[test]
fn redirect_out_on_subshell() {
    assert_eq!(run("( echo x ) > /tmp/huck_t_ss\ncat /tmp/huck_t_ss\n").0, "x\n");
}

#[test]
fn redirect_out_on_case() {
    assert_eq!(run("case z in z) echo m;; esac > /tmp/huck_t_case\ncat /tmp/huck_t_case\n").0, "m\n");
}

#[test]
fn herestring_on_while() {
    assert_eq!(run("while read x; do echo \"[$x]\"; done <<< 'one two'\n").0, "[one two]\n");
}

#[test]
fn append_on_brace_group() {
    assert_eq!(run("echo first > /tmp/huck_t_ap\n{ echo second; } >> /tmp/huck_t_ap\ncat /tmp/huck_t_ap\n").0,
               "first\nsecond\n");
}

#[test]
fn stderr_redirect_on_compound() {
    // 2>&1 then capture: error inside a group goes to the redirected stdout file.
    assert_eq!(run("{ echo out; echo err >&2; } > /tmp/huck_t_se 2>&1\ncat /tmp/huck_t_se\n").0, "out\nerr\n");
}

#[test]
fn no_redirect_compound_unchanged() {
    // Regression: a bare compound still works (not wrapped).
    assert_eq!(run("for i in 1 2; do echo $i; done\n").0, "1\n2\n");
}

#[test]
fn capture_with_inner_redirect() {
    // A >file inside a captured compound diverts that line to the file.
    assert_eq!(run("x=$({ echo a; echo b > /tmp/huck_t_cap; }); echo \"[$x]\"\ncat /tmp/huck_t_cap\n").0,
               "[a]\nb\n");
}
