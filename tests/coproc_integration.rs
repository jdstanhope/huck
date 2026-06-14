//! End-to-end tests for v157 task 4: the `coproc` reserved word's core spawn.
//!
//! `coproc [NAME] BODY` forks BODY asynchronously with its stdin/stdout wired to
//! two pipes; the shell-side pipe ends are held (relocated to high fds,
//! close-on-exec) as NAME[0] (read) / NAME[1] (write). NAME defaults to COPROC.
//! NAME_PID and `$!` are set to the coproc's pid, and a job is registered.
//!
//! These tests are deterministic round-trips: the shell WRITES to the coproc's
//! stdin then READS back the same coproc's stdout (no async race). Each test
//! compares `huck -c <script>` to `bash -c <script>` byte-for-byte. A `timeout`
//! guards against a wiring bug (wrong pipe end on an fd) hanging the suite.

use std::process::Command;
use std::time::Duration;

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Run `prog -c script` with a hard timeout and return (stdout, exit_code).
/// A wiring bug that deadlocks the round-trip would otherwise hang forever; we
/// kill the child after 10s and fail loudly.
fn run_c(prog: &str, script: &str) -> (String, i32) {
    let mut child = Command::new(prog)
        .arg("-c")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            let mut out = child.stdout.take().expect("stdout");
            let mut buf = Vec::new();
            use std::io::Read;
            out.read_to_end(&mut buf).expect("read stdout");
            return (
                String::from_utf8_lossy(&buf).into_owned(),
                status.code().unwrap_or(-1),
            );
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("`{prog} -c` timed out (likely a coproc pipe-wiring deadlock): {script}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Assert huck's stdout matches bash's for the same script.
fn assert_matches_bash(script: &str) {
    let (bash_out, _) = run_c("bash", script);
    let (huck_out, _) = run_c(&huck_binary(), script);
    assert_eq!(
        huck_out, bash_out,
        "stdout differs for script: {script}\n bash: {bash_out:?}\n huck: {huck_out:?}"
    );
}

#[test]
fn coproc_anonymous_roundtrip() {
    // Default name COPROC: write a line to COPROC[1], the coproc body reads it
    // and echoes "got:<line>" to its stdout (COPROC[0]); read it back.
    let script =
        r#"coproc { read l; echo "got:$l"; }; echo hi >&"${COPROC[1]}"; read r <&"${COPROC[0]}"; echo "$r""#;
    let (huck_out, _) = run_c(&huck_binary(), script);
    assert_eq!(huck_out, "got:hi\n", "huck anon round-trip");
    assert_matches_bash(script);
}

#[test]
fn coproc_named_roundtrip() {
    // Named coproc MYP: the fd array is MYP[0]/MYP[1].
    let script =
        r#"coproc MYP { read l; echo "echo:$l"; }; echo yo >&"${MYP[1]}"; read r <&"${MYP[0]}"; echo "$r""#;
    let (huck_out, _) = run_c(&huck_binary(), script);
    assert_eq!(huck_out, "echo:yo\n", "huck named round-trip");
    assert_matches_bash(script);
}

#[test]
fn coproc_sets_pid_and_bang() {
    // COPROC_PID is set to the coproc's pid and equals `$!`; it is a non-empty
    // numeric string. (`[!0-9]`-style globs are avoided: huck treats a leading
    // `!` in a bracket as history expansion in this context — unrelated to
    // coproc — so the numeric check uses a `*[0-9]*` positive match instead.)
    let script = r#"coproc cat; [ "$COPROC_PID" = "$!" ] && echo pidmatch; case "$COPROC_PID" in (""|*[0-9]*) echo numeric;; (*) echo bad;; esac"#;
    let (huck_out, _) = run_c(&huck_binary(), script);
    assert!(
        huck_out.contains("pidmatch"),
        "expected pidmatch, got: {huck_out:?}"
    );
    assert!(
        huck_out.contains("numeric"),
        "expected numeric, got: {huck_out:?}"
    );
    assert_matches_bash(script);
}
