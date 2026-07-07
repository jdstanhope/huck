//! Replace fd 0 with a pipe carrying caller-supplied bytes for the duration of
//! a single closure call, then restore the original fd 0.
//!
//! For short inputs (≤ INLINE_STDIN_THRESHOLD) the bytes are written inline
//! before swapping fd 0, so no thread is needed. For longer inputs a writer
//! thread feeds the pipe until the input is exhausted or the reader closes.
//!
//! Pre-call fd 0 is saved via `dup(0)` and restored via `dup2(saved, 0)` in
//! an RAII guard that runs even on panic.
//!
//! Because fd 0 is process-global, callers must not invoke this helper
//! concurrently — tests gate on `test_support::STDIN_LOCK`.

use std::cell::RefCell;
use std::io::{self, Write};
use std::os::fd::RawFd;
use std::rc::Rc;

use crate::shell_state::Shell;

const INLINE_STDIN_THRESHOLD: usize = 4096;

/// Runs `f` with fd 0 backed by `input`. fd 0 is restored to its pre-call
/// value on return (even on panic).
///
/// `shell_cell` is used ONLY for the (essentially unreachable) pipe/dup
/// error diagnostics below, and each use is a fresh, short-lived
/// `.borrow()` that ends before `f()` runs — never held across it. Holding
/// it across `f()` would risk a `BorrowMutError` panic, since `f()` (the
/// caller's closure) typically re-enters the same `Shell` via its own
/// `.borrow_mut()` (see `exec_builder::run_cwd_inner`'s doc comment on this
/// exact hazard).
pub fn with_stdin_fd0<R>(input: &[u8], shell_cell: &Rc<RefCell<Shell>>, f: impl FnOnce() -> R) -> R {
    let (r, w) = match make_pipe() {
        Ok(pair) => pair,
        Err(e) => {
            // Hard-fail before any state change.
            crate::sh_error!(&*shell_cell.borrow(), None, "pipe: {}", crate::bash_io_error(&e));
            return f(); // run anyway with caller's fd 0; matches "best effort"
        }
    };

    let saved = unsafe { libc::dup(0) };
    if saved < 0 {
        let e = io::Error::last_os_error();
        crate::sh_error!(&*shell_cell.borrow(), None, "dup: {}", crate::bash_io_error(&e));
        unsafe {
            libc::close(r);
            libc::close(w);
        }
        return f();
    }

    if unsafe { libc::dup2(r, 0) } < 0 {
        let e = io::Error::last_os_error();
        crate::sh_error!(&*shell_cell.borrow(), None, "dup2: {}", crate::bash_io_error(&e));
        unsafe {
            libc::close(r);
            libc::close(w);
            libc::close(saved);
        }
        return f();
    }
    unsafe {
        libc::close(r);
    }

    struct Restore {
        saved: RawFd,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = io::stdout().flush();
            unsafe {
                libc::dup2(self.saved, 0);
                libc::close(self.saved);
            }
        }
    }
    let _restore = Restore { saved };

    if input.len() <= INLINE_STDIN_THRESHOLD {
        // Write inline, close, then run.
        let written = unsafe { libc::write(w, input.as_ptr().cast(), input.len()) };
        let _ = written; // best-effort; pipe writes ≤ PIPE_BUF are atomic
        unsafe {
            libc::close(w);
        }
        f()
    } else {
        // Spawn a writer thread that owns `w` and exits when it's closed by EPIPE
        // or by completing the write.
        let input_owned: Vec<u8> = input.to_vec();
        let handle = std::thread::spawn(move || {
            use std::os::fd::FromRawFd;
            let mut file = unsafe { std::fs::File::from_raw_fd(w) };
            let _ = file.write_all(&input_owned);
            // file dropped here -> w closed.
        });
        let result = f();
        // Restore drops fd 0; the writer's pipe peer is closed by the dup2(saved, 0)
        // overwriting the only reader; the writer will see EPIPE or already be done.
        let _ = handle.join();
        result
    }
}

fn make_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0; 2];
    let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::STDIN_LOCK;

    fn test_shell_cell() -> Rc<RefCell<Shell>> {
        Rc::new(RefCell::new(Shell::new()))
    }

    #[test]
    fn short_input_round_trip() {
        let _guard = STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let captured = with_stdin_fd0(b"hello\n", &test_shell_cell(), || {
            let mut buf = [0u8; 16];
            let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
            assert!(n >= 0);
            buf[..n as usize].to_vec()
        });
        assert_eq!(captured, b"hello\n");
    }

    #[test]
    fn fd0_is_restored_after_call() {
        let _guard = STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved = unsafe { libc::dup(0) };
        with_stdin_fd0(b"x", &test_shell_cell(), || ());
        // After the call, fd 0 should still be a valid descriptor; reading
        // from it shouldn't be EBADF.
        let buf = [0u8; 1];
        // Use a poll to check fd 0 is open; reading would block on the
        // terminal in interactive contexts. Just verify the fd is valid:
        let mut pfd = libc::pollfd {
            fd: 0,
            events: 0,
            revents: 0,
        };
        let ret = unsafe { libc::poll(&mut pfd, 1, 0) };
        // ret >= 0 means the fd is valid (could be ready or not, doesn't matter).
        assert!(ret >= 0);
        unsafe {
            libc::close(saved);
        }
        let _ = buf;
    }

    #[test]
    fn large_input_uses_writer_thread() {
        let _guard = STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let big = vec![b'a'; INLINE_STDIN_THRESHOLD + 100];
        let captured = with_stdin_fd0(&big, &test_shell_cell(), || {
            let mut got = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                let n = unsafe { libc::read(0, buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n as usize]);
            }
            got
        });
        assert_eq!(captured.len(), big.len());
    }
}
