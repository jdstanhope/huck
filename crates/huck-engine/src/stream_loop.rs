//! Shared external-process capture helper. Replaces the blocking-waitpid +
//! drainer-thread shape from v205 with a poll-based loop on the embedder's
//! thread. Reads pipe bytes as they arrive, line-buffers into the active
//! Callbacks (via thread-local), AND appends bytes to a capture buffer
//! for the tee with Output.stdout/stderr.

use crate::callbacks_thread_local::with_callbacks;
use crate::wait_loop::{Event, WaitLoop};
use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

const CHUNK_SIZE: usize = 8192;

pub struct CaptureSinks<'a> {
    /// Where stdout bytes go (Some for capture; None to discard).
    pub stdout: Option<&'a mut Vec<u8>>,
    /// Where stderr bytes go.
    pub stderr: Option<&'a mut Vec<u8>>,
}

/// Wait for `child_pid` to exit, polling `pipe_out` and `pipe_err`. Pipe
/// reads are pushed into the capture sinks AND dispatched via the active
/// Callbacks thread-local (line-buffered). Returns the child's raw wait
/// status (as `libc::waitpid`'s status integer).
///
/// `pipe_out` / `pipe_err` may be -1 if not in use (e.g. when stderr is
/// inherited or merged onto stdout).
pub fn external_capture_loop(
    child_pid: libc::pid_t,
    pipe_out: RawFd,
    pipe_err: RawFd,
    sinks: CaptureSinks<'_>,
    mut timeout_remaining: impl FnMut() -> Option<Duration>,
) -> io::Result<i32> {
    // #120: With no capture pipes to stream AND no embedder deadline, there is
    // nothing for the poll loop to watch — it would fall back to sleeping
    // POLL_TICK_MS (100ms) before each reap, so every foreground external
    // command / subshell with inherited stdio paid ~100ms. Block on the child
    // directly instead. Behavior-equivalent for signals/traps (both re-wait on
    // EINTR); no pipes means no final drain is needed.
    if pipe_out < 0 && pipe_err < 0 && timeout_remaining().is_none() {
        return blocking_wait(child_pid);
    }
    let mut wl = WaitLoop::new()?;
    if pipe_out >= 0 {
        set_nonblock(pipe_out)?;
        wl.register_pipe(pipe_out)?;
    }
    if pipe_err >= 0 {
        set_nonblock(pipe_err)?;
        wl.register_pipe(pipe_err)?;
    }
    // Note: we INTENTIONALLY do NOT use signalfd/SIGCHLD as the only wakeup
    // source. In a multi-threaded process (e.g. the cargo test runtime, or any
    // embedder that spawns auxiliary threads) SIGCHLD may be delivered to a
    // thread whose default disposition is "ignore", consuming the signal
    // before our signalfd can read it. Instead we poll the pipes with a short
    // timeout and waitpid(WNOHANG) the child on each tick. This is robust
    // regardless of thread topology and adds at most ~POLL_TICK latency to
    // the child-exit observation.
    const POLL_TICK_MS: i32 = 100;
    let poll_tick = Duration::from_millis(POLL_TICK_MS as u64);

    let mut sinks = sinks;
    loop {
        // Check whether the child has exited.
        let mut status: i32 = 0;
        let wpid = unsafe { libc::waitpid(child_pid, &mut status, libc::WNOHANG) };
        if wpid == child_pid {
            // Drain final bytes from both pipes (child may have written right
            // before exit).
            if pipe_out >= 0 {
                drain_to_eof(pipe_out, sinks.stdout.as_deref_mut(), true)?;
            }
            if pipe_err >= 0 {
                drain_to_eof(pipe_err, sinks.stderr.as_deref_mut(), false)?;
            }
            return Ok(status);
        }
        // Sleep until pipes are readable, the embedder-supplied timeout fires,
        // or POLL_TICK_MS elapses (so we re-check the child).
        let to = match timeout_remaining() {
            Some(d) if d < poll_tick => d,
            _ => poll_tick,
        };
        let events = wl.poll(Some(to))?;
        if pipe_out < 0 && pipe_err < 0 {
            // No pipes registered: poll has no fd to watch and returns
            // immediately on Linux. Avoid busy-spinning by sleeping the tick.
            if events.is_empty() {
                std::thread::sleep(to);
            }
            continue;
        }
        for ev in events {
            match ev {
                Event::Readable(fd) if fd == pipe_out => {
                    read_and_dispatch(fd, sinks.stdout.as_deref_mut(), true)?;
                }
                Event::Readable(fd) if fd == pipe_err => {
                    read_and_dispatch(fd, sinks.stderr.as_deref_mut(), false)?;
                }
                Event::Readable(_) | Event::ChildExited => {}
            }
        }
    }
}

/// Block until `child_pid` exits, retrying on `EINTR` so a signal delivered to
/// the shell (e.g. a trap) is handled and the wait resumes. Returns the raw
/// `waitpid` status. Used by `external_capture_loop`'s no-pipe / no-timeout
/// fast path, where there is nothing to stream. Flags `0` (no `WUNTRACED`)
/// match the poll loop it replaces; foreground job-control stop handling lives
/// on the interactive path, not here.
fn blocking_wait(child_pid: libc::pid_t) -> io::Result<i32> {
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

/// Pipeline variant: poll `pipe_out` and `pipe_err` until BOTH have returned
/// EOF (or the call hits an unrecoverable error). Does NOT waitpid — the
/// caller is responsible for reaping the pipeline's stages afterward (e.g.
/// via `wait_pipeline_raw`). Bytes are dispatched the same way as
/// `external_capture_loop`: appended to the optional capture sink AND pushed
/// to the active Callbacks thread-local.
///
/// This shape is needed for multi-stage pipelines where there's no single
/// child pid to gate on — the last stage's exit closes `pipe_out`, but the
/// shared stderr pipe stays open until every stage has exited. Polling on
/// EOF lets us see "all writers gone" without prematurely returning.
pub fn pipeline_drain_loop(
    pipe_out: RawFd,
    pipe_err: RawFd,
    sinks: CaptureSinks<'_>,
) -> io::Result<()> {
    let mut wl = WaitLoop::new()?;
    let mut out_eof = pipe_out < 0;
    let mut err_eof = pipe_err < 0;
    if pipe_out >= 0 {
        set_nonblock(pipe_out)?;
        wl.register_pipe(pipe_out)?;
    }
    if pipe_err >= 0 {
        set_nonblock(pipe_err)?;
        wl.register_pipe(pipe_err)?;
    }
    // No SIGCHLD — pipeline reaper handles waitpid separately.

    let mut sinks = sinks;
    while !out_eof || !err_eof {
        let events = wl.poll(None)?;
        if events.is_empty() {
            continue;
        }
        for ev in events {
            match ev {
                Event::Readable(fd) if fd == pipe_out && !out_eof => {
                    let eof = read_and_dispatch_eof(fd, sinks.stdout.as_deref_mut(), true)?;
                    if eof {
                        out_eof = true;
                        wl.unregister_pipe(fd);
                    }
                }
                Event::Readable(fd) if fd == pipe_err && !err_eof => {
                    let eof = read_and_dispatch_eof(fd, sinks.stderr.as_deref_mut(), false)?;
                    if eof {
                        err_eof = true;
                        wl.unregister_pipe(fd);
                    }
                }
                Event::Readable(_) | Event::ChildExited => {}
            }
        }
    }
    Ok(())
}

/// Like `read_and_dispatch` but reports whether EOF was seen (read returned 0).
fn read_and_dispatch_eof(
    fd: RawFd,
    mut sink: Option<&mut Vec<u8>>,
    is_stdout: bool,
) -> io::Result<bool> {
    let mut buf = [0u8; CHUNK_SIZE];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            let chunk = &buf[..n as usize];
            if let Some(sink) = sink.as_deref_mut() {
                sink.extend_from_slice(chunk);
            }
            with_callbacks(|cb| {
                if let Some(cb) = cb {
                    if is_stdout {
                        cb.push_stdout(chunk);
                    } else {
                        cb.push_stderr(chunk);
                    }
                }
            });
            if (n as usize) < CHUNK_SIZE {
                return Ok(false);
            }
        } else if n == 0 {
            return Ok(true);
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN)
                || err.raw_os_error() == Some(libc::EWOULDBLOCK)
            {
                return Ok(false);
            }
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
    }
}

fn set_nonblock(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a valid fd is a stable libc call.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn read_and_dispatch(fd: RawFd, mut sink: Option<&mut Vec<u8>>, is_stdout: bool) -> io::Result<()> {
    let mut buf = [0u8; CHUNK_SIZE];
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            let chunk = &buf[..n as usize];
            if let Some(sink) = sink.as_deref_mut() {
                sink.extend_from_slice(chunk);
            }
            with_callbacks(|cb| {
                if let Some(cb) = cb {
                    if is_stdout {
                        cb.push_stdout(chunk);
                    } else {
                        cb.push_stderr(chunk);
                    }
                }
            });
            if (n as usize) < CHUNK_SIZE {
                return Ok(());
            }
        } else if n == 0 {
            return Ok(());
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN)
                || err.raw_os_error() == Some(libc::EWOULDBLOCK)
            {
                return Ok(());
            }
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
    }
}

fn drain_to_eof(fd: RawFd, sink: Option<&mut Vec<u8>>, is_stdout: bool) -> io::Result<()> {
    let mut buf = [0u8; CHUNK_SIZE];
    let mut sink = sink;
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            let chunk = &buf[..n as usize];
            if let Some(sink) = sink.as_deref_mut() {
                sink.extend_from_slice(chunk);
            }
            with_callbacks(|cb| {
                if let Some(cb) = cb {
                    if is_stdout {
                        cb.push_stdout(chunk);
                    } else {
                        cb.push_stderr(chunk);
                    }
                }
            });
        } else if n == 0 {
            return Ok(());
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            // EAGAIN on a non-blocking pipe means no more data is available
            // RIGHT NOW. Since the child has already exited (we're in the
            // final-drain phase), no more writes can ever arrive; treat as
            // EOF. Matches kqueue/poll semantics where the pipe is closed
            // on the writer side after the writer process exits.
            if err.raw_os_error() == Some(libc::EAGAIN)
                || err.raw_os_error() == Some(libc::EWOULDBLOCK)
            {
                return Ok(());
            }
            return Err(err);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn no_pipe_wait_is_prompt_and_correct() {
        // Fork a child that exits(7) immediately. The no-pipe / no-timeout
        // fast path must return its status without the old ~100ms poll-tick
        // latency (regression guard for #120).
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe { libc::_exit(7) };
        }
        let sinks = CaptureSinks {
            stdout: None,
            stderr: None,
        };
        let start = Instant::now();
        let status = external_capture_loop(pid, -1, -1, sinks, || None).unwrap();
        let elapsed = start.elapsed();
        assert!(libc::WIFEXITED(status), "child did not exit normally");
        assert_eq!(libc::WEXITSTATUS(status), 7, "wrong exit status");
        assert!(
            elapsed < Duration::from_millis(50),
            "no-pipe wait took {elapsed:?}; expected prompt return (#120 regression)"
        );
    }
}
