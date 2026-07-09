//! v134: heredoc/herestring bodies are fed by a forked writer, so large bodies
//! (> pipe buffer) and backpressuring consumers no longer deadlock (M-120).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// SIGKILL the entire descendant tree rooted at `root` (BFS via `pgrep -P`),
/// then `root` itself. A hung huck spawns its pipeline stages in their OWN
/// process group (pgid = first-stage pid, distinct from huck's pid/group), so
/// killing huck alone orphans those stages — they linger blocked on pipes
/// (reparented to PID 1). Killing the whole tree prevents the leak.
fn kill_process_tree(root: u32) {
    let mut pids = vec![root];
    let mut i = 0;
    while i < pids.len() {
        let pid = pids[i];
        i += 1;
        if let Ok(o) = Command::new("pgrep")
            .arg("-P")
            .arg(pid.to_string())
            .output()
        {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Ok(child) = line.trim().parse::<u32>() {
                    pids.push(child);
                }
            }
        }
    }
    for pid in pids {
        // SAFETY: `kill(pid, SIGKILL)` is an always-safe syscall; an already-dead
        // or reparented pid just yields ESRCH, which we ignore.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

fn run_guarded(script: &str, secs: u64) -> Option<(String, String, i32)> {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    let pid = child.id();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let (tx, rx) = mpsc::channel::<()>();
    let wd = thread::spawn(move || -> bool {
        if rx.recv_timeout(Duration::from_secs(secs)).is_err() {
            kill_process_tree(pid);
            true
        } else {
            false
        }
    });
    let out = child.wait_with_output().unwrap();
    let _ = tx.send(());
    if wd.join().unwrap() {
        None
    } else {
        Some((
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        ))
    }
}
fn with_big_v(frag: &str) -> String {
    format!("V=$(printf 'x%.0s' $(seq 1 200000))\n{frag}\n")
}

#[test]
fn compound_heredoc_large_body() {
    let (o, _e, _c) = run_guarded(&with_big_v("{ wc -c; } << EOF\n$V\nEOF"), 10)
        .expect("HUNG: compound heredoc deadlocked");
    assert_eq!(o.trim(), "200001", "o: {o:?}");
}
#[test]
fn compound_awk_while_heredoc_nvm_shape() {
    let (o, _e, _c) = run_guarded(
        &with_big_v("{ command awk '{print}' | wc -l; } << EOF\n$V\nEOF"),
        10,
    )
    .expect("HUNG: awk|while compound heredoc deadlocked");
    assert_eq!(o.trim(), "1", "o: {o:?}");
}
#[test]
fn compound_herestring_large_body() {
    let (o, _e, _c) = run_guarded(&with_big_v("{ wc -c; } <<< \"$V\""), 10)
        .expect("HUNG: compound herestring deadlocked");
    assert_eq!(o.trim(), "200001", "o: {o:?}");
}
#[test]
fn small_compound_heredoc_no_regression() {
    let (o, _e, _c) = run_guarded("{ cat; } << EOF\nyo\nEOF\n", 10).expect("hung");
    assert_eq!(o, "yo\n", "o: {o:?}");
}

#[test]
fn pipeline_heredoc_large_body() {
    let (o, _e, _c) = run_guarded(&with_big_v("cat << EOF | wc -c\n$V\nEOF"), 10)
        .expect("HUNG: pipeline heredoc deadlocked");
    assert_eq!(o.trim(), "200001", "o: {o:?}");
}
#[test]
fn captured_single_heredoc_large_body() {
    let (o, _e, _c) = run_guarded(&with_big_v("r=$(cat << EOF\n$V\nEOF\n); echo ${#r}"), 10)
        .expect("HUNG: captured single-command heredoc deadlocked");
    assert_eq!(o.trim(), "200000", "o: {o:?}");
}
#[test]
fn small_pipeline_heredoc_no_regression() {
    let (o, _e, _c) = run_guarded("cat << EOF | wc -c\nhi\nEOF\n", 10).expect("hung");
    assert_eq!(o.trim(), "3", "o: {o:?}");
}
#[test]
fn dollar_bang_unaffected_by_heredoc() {
    let (o, _e, _c) = run_guarded(
        "sleep 0.2 & p=$!; cat << EOF >/dev/null\nx\nEOF\necho \"$p $!\"\n",
        10,
    )
    .expect("hung");
    let parts: Vec<&str> = o.split_whitespace().collect();
    assert_eq!(parts.len(), 2, "o: {o:?}");
    assert_eq!(parts[0], parts[1], "heredoc writer changed $!; o: {o:?}");
}
