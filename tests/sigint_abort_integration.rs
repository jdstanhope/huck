//! v138: an untrapped SIGINT aborts the running command list / function / script
//! like bash. Deterministic via `kill -INT $$` (sets huck's own sigint_flag — no
//! PTY/timing). Runs the huck binary as a subprocess.
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `huck -c <script>`; return (stdout, stderr, exit_code).
fn huck_c(script: &str) -> (String, String, i32) {
    let o = Command::new(huck_bin())
        .arg("-c").arg(script)
        .stdin(Stdio::null())
        .output()
        .expect("spawn huck");
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

#[test]
fn sequence_aborts_on_sigint() {
    let (out, _e, code) = huck_c("echo a; kill -INT $$; echo b");
    assert_eq!(out, "a\n", "second command must not run; out={out:?}");
    assert_eq!(code, 130, "exit 130 expected");
}

#[test]
fn function_body_aborts_and_unwinds_caller() {
    let (out, _e, code) = huck_c("f(){ echo a; kill -INT $$; echo b; }; f; echo after");
    assert_eq!(out, "a\n", "abort unwinds through the function AND the caller; out={out:?}");
    assert_eq!(code, 130);
}

#[test]
fn nested_if_aborts() {
    let (out, _e, code) = huck_c("if true; then echo a; kill -INT $$; echo b; fi; echo c");
    assert_eq!(out, "a\n", "out={out:?}");
    assert_eq!(code, 130);
}

#[test]
fn trap_handler_runs_and_continues() {
    let (out, _e, code) = huck_c("trap 'echo c' INT; echo a; kill -INT $$; echo b");
    assert_eq!(out, "a\nc\nb\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn trap_ignore_continues() {
    let (out, _e, code) = huck_c("trap '' INT; echo a; kill -INT $$; echo b");
    assert_eq!(out, "a\nb\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn legit_130_status_does_not_abort() {
    let (out, _e, code) = huck_c("f(){ return 130; }; f; echo still-here");
    assert_eq!(out, "still-here\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn loop_aborts_and_unwinds_sequence() {
    // The loop aborts AND the trailing `echo after` must not run.
    let (out, _e, code) = huck_c("for i in 1 2 3; do echo $i; kill -INT $$; done; echo after");
    assert_eq!(out, "1\n", "out={out:?}");
    assert_eq!(code, 130);
}

#[test]
fn while_read_loop_aborts() {
    // The loop must abort after the first iteration AND `echo after` must not
    // run. The loop reads from a heredoc (not a `seq | while` pipe): a piped
    // loop runs in a forked subshell, so `kill -INT $$` targets the PARENT and
    // never reaches the subshell's flag — bash itself runs all 3 iterations
    // there. Feeding via heredoc keeps the loop in the main shell, which is the
    // path the between-iteration `check_interrupt` guards (bash-identical: `1`,
    // rc 130).
    let (out, _e, code) =
        huck_c("while read x; do echo $x; kill -INT $$; done <<EOF\n1\n2\n3\nEOF\necho after");
    assert_eq!(out, "1\n", "out={out:?}");
    assert_eq!(code, 130);
}

#[test]
fn command_substitution_aborts() {
    // SIGINT inside $(...) aborts; the trailing command must not run.
    let (out, _e, code) = huck_c("x=$(echo a; kill -INT $$; echo b); echo \"[$x]\"; echo after");
    assert!(!out.contains("after"), "must abort before `after`; out={out:?}");
    assert_eq!(code, 130, "out={out:?}");
}
