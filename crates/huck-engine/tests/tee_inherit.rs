//! fd-inheritance ("tee") checks for the streaming line callbacks, in their
//! OWN test binary so they never share a process with other forking tests.
//!
//! Each check swaps a process-global standard fd (1 or 2) for a pipe around a
//! `fork`+`exec`, runs a script, then restores the fd and reads what the pipe
//! captured — verifying the streaming tee ALSO re-writes each line to the
//! embedder's real fd (not just the callback). Because `dup2` clears
//! `O_CLOEXEC`, any concurrently forking test in the same process would
//! inherit the temporarily-installed pipe and clobber the capture. Isolating
//! them in this integration binary (its own process) removes that race; the
//! two checks run sequentially in one `#[test]` so they don't race each other
//! either. See #90.

use std::io::Read;
use std::os::fd::FromRawFd;

use huck_engine::Engine;

/// Swap `target_fd` (1 or 2) for a pipe, run `script` with the matching line
/// callback, restore the fd, and return `(callback_lines, piped_bytes)`.
fn run_with_fd_capture(target_fd: i32, script: &str, on_stderr: bool) -> (Vec<String>, String) {
    let mut fds = [0; 2];
    let r = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    assert_eq!(r, 0, "pipe2 failed");
    let pipe_r = fds[0];
    let pipe_w = fds[1];

    let saved = unsafe { libc::dup(target_fd) };
    unsafe {
        libc::dup2(pipe_w, target_fd);
        libc::close(pipe_w);
    }

    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    {
        let builder = e.exec(script);
        let builder = if on_stderr {
            builder.on_stderr_line(|line| lines.push(line.to_string()))
        } else {
            builder.on_stdout_line(|line| lines.push(line.to_string()))
        };
        let _ = builder.run();
    }

    unsafe {
        libc::dup2(saved, target_fd);
        libc::close(saved);
    }

    let mut buf = String::new();
    let mut file = unsafe { std::fs::File::from_raw_fd(pipe_r) };
    file.read_to_string(&mut buf).unwrap();

    (lines, buf)
}

#[test]
fn tee_inherits_std_fds() {
    // fd 1: the stdout callback fires AND the embedder's real fd 1 sees the line.
    let (lines, buf) = run_with_fd_capture(1, "echo tee-hi", false);
    assert_eq!(lines, vec!["tee-hi"]);
    assert_eq!(
        buf, "tee-hi\n",
        "embedder's real fd 1 should also see the line"
    );

    // fd 2: same, for the stderr callback.
    let (lines, buf) = run_with_fd_capture(2, "echo err >&2", true);
    assert_eq!(lines, vec!["err"]);
    assert_eq!(
        buf, "err\n",
        "embedder's real fd 2 should also see the line"
    );
}
