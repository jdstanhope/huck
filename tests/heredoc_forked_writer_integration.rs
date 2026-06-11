//! v134: heredoc/herestring bodies are fed by a forked writer, so large bodies
//! (> pipe buffer) and backpressuring consumers no longer deadlock (M-120).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run_guarded(script: &str, secs: u64) -> Option<(String, String, i32)> {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    let pid = child.id();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let (tx, rx) = mpsc::channel::<()>();
    let wd = thread::spawn(move || -> bool {
        if rx.recv_timeout(Duration::from_secs(secs)).is_err() {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            true
        } else { false }
    });
    let out = child.wait_with_output().unwrap();
    let _ = tx.send(());
    if wd.join().unwrap() { None } else {
        Some((String::from_utf8_lossy(&out.stdout).into_owned(),
              String::from_utf8_lossy(&out.stderr).into_owned(),
              out.status.code().unwrap_or(-1)))
    }
}
fn with_bigV(frag: &str) -> String {
    format!("V=$(printf 'x%.0s' $(seq 1 200000))\n{frag}\n")
}

#[test]
fn compound_heredoc_large_body() {
    let (o, _e, _c) = run_guarded(&with_bigV("{ wc -c; } << EOF\n$V\nEOF"), 10)
        .expect("HUNG: compound heredoc deadlocked");
    assert_eq!(o.trim(), "200001", "o: {o:?}");
}
#[test]
fn compound_awk_while_heredoc_nvm_shape() {
    let (o, _e, _c) = run_guarded(&with_bigV("{ command awk '{print}' | wc -l; } << EOF\n$V\nEOF"), 10)
        .expect("HUNG: awk|while compound heredoc deadlocked");
    assert_eq!(o.trim(), "1", "o: {o:?}");
}
#[test]
fn compound_herestring_large_body() {
    let (o, _e, _c) = run_guarded(&with_bigV("{ wc -c; } <<< \"$V\""), 10)
        .expect("HUNG: compound herestring deadlocked");
    assert_eq!(o.trim(), "200001", "o: {o:?}");
}
#[test]
fn small_compound_heredoc_no_regression() {
    let (o, _e, _c) = run_guarded("{ cat; } << EOF\nyo\nEOF\n", 10).expect("hung");
    assert_eq!(o, "yo\n", "o: {o:?}");
}
