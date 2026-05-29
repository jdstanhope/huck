use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
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
fn jobs_p_outputs_bg_pid() {
    // Spawn a bg sleep, capture $!, then run `jobs -p` and verify the
    // pgid printed by jobs -p matches the bg PID. The sleep redirects
    // stdio so wait_with_output() returns promptly when huck exits.
    let script = "sleep 30 >/dev/null 2>&1 &\nbg=$!\njobs -p\necho LAST=$bg\nexit\n";
    let (out, _) = run_capture(script);
    let mut jobs_p_pid: Option<i32> = None;
    let mut last_pid: Option<i32> = None;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("LAST=") {
            if let Ok(n) = rest.parse::<i32>() {
                last_pid = Some(n);
            }
        } else if let Ok(n) = line.trim().parse::<i32>()
            && jobs_p_pid.is_none()
            && n > 0
        {
            jobs_p_pid = Some(n);
        }
    }
    let jp = jobs_p_pid.unwrap_or_else(|| panic!("no jobs -p pid in: {:?}", out));
    let lp = last_pid.unwrap_or_else(|| panic!("no LAST= line in: {:?}", out));
    assert_eq!(jp, lp, "jobs -p pid ({jp}) != $! ({lp})");
}

#[test]
fn jobs_l_includes_pid_in_listing() {
    // `jobs -l` output should contain the bg PID plus the [N] job tag.
    let script = "sleep 30 >/dev/null 2>&1 &\nbg=$!\njobs -l\necho LAST=$bg\nexit\n";
    let (out, _) = run_capture(script);
    let mut last_pid: Option<i32> = None;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("LAST=")
            && let Ok(n) = rest.parse::<i32>()
        {
            last_pid = Some(n);
        }
    }
    let lp = last_pid.unwrap_or_else(|| panic!("no LAST= line in: {:?}", out));
    assert!(
        out.contains(&format!("{lp}")),
        "jobs -l output missing pid {lp}: {:?}",
        out
    );
    assert!(out.contains("[1]"), "jobs -l missing [1] tag: {:?}", out);
}
