use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

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

fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn cleanup_kill(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

fn first_pid(s: &str) -> Option<i32> {
    for word in s.split_whitespace() {
        if let Ok(n) = word.parse::<i32>()
            && n > 0
        {
            return Some(n);
        }
    }
    None
}

#[test]
fn disown_h_with_bare_pid_lets_bg_survive() {
    // sleep redirects stdio so wait_with_output() returns promptly
    // when huck exits. `echo $!` captures the bg PID. `disown -h $!`
    // uses the bare-PID path added in v44 task 1.
    let script = "sleep 30 >/dev/null 2>&1 &\necho $!\ndisown -h $!\nexit\n";
    let (out, _) = run_capture(script);
    let pid = first_pid(&out).unwrap_or_else(|| panic!("no pid found in: {:?}", out));
    thread::sleep(Duration::from_millis(200));
    let alive = pid_alive(pid);
    cleanup_kill(pid);
    assert!(
        alive,
        "bg job (pid {pid}) was killed despite disown -h <pid>"
    );
}
