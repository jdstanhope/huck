//! v137: SIGPIPE is restored to SIG_DFL process-wide, so a producer writing to
//! a closed pipe dies silently (status 141) like bash instead of looping on
//! EPIPE and spamming "Broken pipe". Tests run the huck binary as a subprocess
//! (resetting SIGPIPE in the test process would not affect a spawned child).
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `huck -c <script>` with no stdin; return (stdout, stderr, exit_code).
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

// A forked builtin producer whose consumer reads one line and exits must die on
// SIGPIPE: the producer stage status is 141 and NOTHING is printed to stderr.
#[test]
fn forked_producer_status_141_silent() {
    let (out, err, code) = huck_c(
        "{ for i in $(seq 1 5000); do echo $i; done; } | { read x; }; echo \"stages=${PIPESTATUS[*]}\"",
    );
    assert_eq!(code, 0, "overall rc; stderr={err:?}");
    assert_eq!(out, "stages=141 0\n", "producer must be SIGPIPE-killed (141); out={out:?}");
    assert_eq!(err, "", "no Broken pipe spam expected; err={err:?}");
}

// A 5000-line producer into `head -1` must emit exactly one line and ZERO
// "Broken pipe" lines on stderr (the assertion the fix fired).
#[test]
fn forked_producer_no_broken_pipe_spam() {
    let (out, err, _code) = huck_c(
        "for i in $(seq 1 5000); do echo \"line$i\"; done | head -1",
    );
    assert_eq!(out, "line1\n", "out={out:?}");
    assert!(!err.contains("Broken pipe"), "stderr leaked Broken pipe: {err:?}");
}

// A subshell `( ... )` producer is a forked stage; it too must die silently.
#[test]
fn subshell_producer_status_141_silent() {
    let (out, err, code) = huck_c(
        "( for i in $(seq 1 5000); do echo $i; done ) | { read x; }; echo \"stages=${PIPESTATUS[*]}\"",
    );
    assert_eq!(code, 0, "stderr={err:?}");
    assert_eq!(out, "stages=141 0\n", "out={out:?}");
    assert_eq!(err, "", "err={err:?}");
}

// A shell function producer (runs in the forked stage) must die silently too.
#[test]
fn function_producer_no_spam() {
    let (out, err, _c) = huck_c(
        "f(){ local i=0; while [ \"$i\" -lt 5000 ]; do echo \"$i\"; i=$((i+1)); done; }; f | head -2",
    );
    assert_eq!(out, "0\n1\n", "out={out:?}");
    assert!(!err.contains("Broken pipe"), "err={err:?}");
}
