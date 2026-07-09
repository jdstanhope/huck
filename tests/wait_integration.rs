use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
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
    )
}

#[test]
fn wait_multiarg_all_succeed() {
    // Both bg jobs exit 0; wait %1 %2; echo $? → 0.
    // Sleeps keep both jobs alive past huck's between-statement reap+notify
    // cycle so %1/%2 are still resolvable when `wait` runs. (Without them,
    // `true` as a builtin (v53) returns instantly and the jobs reap before
    // `wait` resolves them.)
    let (out, _) = run("(sleep 0.1; true) &\n(sleep 0.2; true) &\nwait %1 %2\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {:?}", out);
}

#[test]
fn wait_multiarg_returns_last_status() {
    // First bg job exits 5, second exits 3. wait %1 %2 → 3. Sleeps keep
    // both jobs alive past huck's between-statement reap+notify cycle so
    // %1 is still resolvable when `wait %1 %2` runs.
    let (out, _) = run("(sleep 0.1; exit 5) &\n(sleep 0.2; exit 3) &\nwait %1 %2\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "3"), "stdout: {:?}", out);
}

#[test]
fn wait_n_returns_first_finished_status() {
    // One bg job sleeps briefly then exits 7. wait -n → 7.
    let (out, _) = run("(sleep 0.05; exit 7) &\nwait -n\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "7"), "stdout: {:?}", out);
}

#[test]
fn wait_n_with_no_jobs_returns_127() {
    let (out, _) = run("wait -n\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "127"), "stdout: {:?}", out);
}

#[test]
fn wait_n_p_captures_pid_into_var() {
    // wait -n -p FINPID against a single bg job; echo $FINPID prints the
    // job's pgid (a positive integer). Asserts pid line parses to a positive
    // int and the exit status line shows the bg job's exit code (3).
    // We capture $? into `rc` immediately after `wait` so the subsequent
    // `echo "pid=..."` doesn't clobber it before the assertion line prints.
    let (out, _) = run(
        "(sleep 0.05; exit 3) &\nwait -n -p FINPID\nrc=$?\necho \"pid=$FINPID\"\necho $rc\nexit\n",
    );
    let mut pid_line = None;
    let mut status_line = None;
    for l in out.lines() {
        if let Some(rest) = l.strip_prefix("pid=") {
            pid_line = Some(rest.to_string());
        } else if l == "3" {
            status_line = Some(l.to_string());
        }
    }
    let pid = pid_line.unwrap_or_else(|| panic!("no pid= line in stdout: {:?}", out));
    let parsed: i32 = pid
        .parse()
        .unwrap_or_else(|_| panic!("pid not an integer: {pid:?}"));
    assert!(parsed > 0, "pid was not positive: {parsed}");
    assert!(status_line.is_some(), "no status line: {:?}", out);
}
