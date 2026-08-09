//! Blocking wait for a foreground external child, on the embedder's thread.
//!
//! This module used to be a poll-based streaming loop: it took two pipe fds and
//! a pair of capture sinks, polled the pipes, and pushed bytes into the sinks as
//! they arrived. That design was superseded by the #197 fd-one-model arc, whose
//! Stage 3 moved capture onto real file descriptors and deleted the software
//! sinks — after which every caller passed `-1, -1` for the pipes and `None` for
//! both sinks, and the streaming half became unreachable (#504).
//!
//! Unreachable, but not free: it kept ~110 lines and a `CaptureSinks` type alive,
//! and reported 20% coverage in a way that read as a testing gap rather than as
//! dead code. The embedder-facing `Callbacks` streaming API it existed for is
//! not coming back, so it is gone.
//!
//! What remains is what the callers actually did: block until the child exits.

use std::io;

/// Block until `child_pid` exits, retrying on `EINTR` so a signal delivered to
/// the shell (e.g. a trap) is handled and the wait resumes. Returns the raw
/// `waitpid` status.
///
/// Flags `0` (no `WUNTRACED`) is deliberate and unchanged from the poll loop
/// this replaced: foreground job-control stop handling lives on the interactive
/// path, not here.
pub fn wait_for_child(child_pid: libc::pid_t) -> io::Result<i32> {
    loop {
        let mut status: i32 = 0;
        let r = unsafe { libc::waitpid(child_pid, &mut status, 0) };
        if r == child_pid {
            return Ok(status);
        }
        if r < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
        // r == 0 is impossible without WNOHANG; loop defensively.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn wait_is_prompt_and_correct() {
        // Fork a child that exits(7) immediately. The wait must return its
        // status without the ~100ms poll-tick latency the old loop imposed
        // (regression guard for #120, kept through the #504 collapse).
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe { libc::_exit(7) };
        }
        let start = Instant::now();
        let status = wait_for_child(pid).unwrap();
        let elapsed = start.elapsed();
        assert!(libc::WIFEXITED(status), "child did not exit normally");
        assert_eq!(libc::WEXITSTATUS(status), 7, "wrong exit status");
        assert!(
            elapsed < Duration::from_millis(50),
            "wait took {elapsed:?}; expected prompt return (#120 regression)"
        );
    }
}
