//! v137: SIGPIPE is restored to SIG_DFL process-wide, so a producer writing to
//! a closed pipe dies silently (status 141) like bash instead of looping on
//! EPIPE and spamming "Broken pipe". Tests run the huck binary as a subprocess
//! (resetting SIGPIPE in the test process would not affect a spawned child).
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run `huck -c <script>` with no stdin; return (stdout, stderr, exit_code).
fn huck_c(script: &str) -> (String, String, i32) {
    let o = Command::new(huck_bin())
        .arg("-c")
        .arg(script)
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
    assert_eq!(
        out, "stages=141 0\n",
        "producer must be SIGPIPE-killed (141); out={out:?}"
    );
    assert_eq!(err, "", "no Broken pipe spam expected; err={err:?}");
}

// A 5000-line producer into `head -1` must emit exactly one line and ZERO
// "Broken pipe" lines on stderr (the assertion the fix fired).
#[test]
fn forked_producer_no_broken_pipe_spam() {
    let (out, err, _code) = huck_c("for i in $(seq 1 5000); do echo \"line$i\"; done | head -1");
    assert_eq!(out, "line1\n", "out={out:?}");
    assert!(
        !err.contains("Broken pipe"),
        "stderr leaked Broken pipe: {err:?}"
    );
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

// A producer in huck's MAIN process (huck's own stdout is the pipe) must die on
// SIGPIPE when the reader closes — terminate, no infinite loop, no spam.
//
// NOTE: huck (like bash) is *killed by* SIGPIPE here — it does not install a
// handler that exit(141)s. So the process is signal-terminated by signal 13,
// and `ExitStatus::code()` is `None` (the familiar 141 = 128+13 is only what an
// *observing parent shell* records in `$?`). Verified byte-for-byte against
// bash: both die by signal 13 with empty stderr.
#[test]
fn main_process_producer_terminates_on_broken_pipe() {
    let mut child = Command::new(huck_bin())
        .arg("-c")
        .arg("while true; do printf 'x\\n'; done")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");

    // Read one byte, then close the read end so huck's next write gets SIGPIPE.
    {
        let mut so = child.stdout.take().unwrap();
        let mut one = [0u8; 1];
        so.read_exact(&mut one).expect("read first byte");
        assert_eq!(&one, b"x");
        // `so` dropped here -> read end closed.
    }

    // Watchdog: huck must exit on its own within a few seconds.
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(st) = child.try_wait().expect("try_wait") {
            break st;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("huck did not terminate on a broken pipe (infinite loop)");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        status.signal(),
        Some(libc::SIGPIPE),
        "expected termination by SIGPIPE (signal 13, i.e. $? 141 in a parent); got {status:?}"
    );

    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).ok();
    assert!(
        !err.contains("Broken pipe"),
        "stderr leaked Broken pipe: {err:?}"
    );
}

// Restoring SIG_DFL at startup makes SIGPIPE trappable again (was rejected with
// "cannot reset ignored signal").
#[test]
fn trap_pipe_is_now_settable() {
    let (out, err, code) = huck_c("trap 'echo handler' PIPE; echo set-ok");
    assert_eq!(out, "set-ok\n", "out={out:?}");
    assert_eq!(code, 0, "code={code}");
    assert!(!err.contains("cannot reset ignored signal"), "err={err:?}");
}
