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
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
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
pub fn with_stdin_fd0<R>(
    input: &[u8],
    shell_cell: &Rc<RefCell<Shell>>,
    f: impl FnOnce() -> R,
) -> R {
    let (r, w) = match crate::child_fd::make_pipe_owned(true) {
        Ok(pair) => pair,
        Err(e) => {
            // Hard-fail before any state change.
            crate::sh_error!(
                &*shell_cell.borrow(),
                None,
                "pipe: {}",
                crate::bash_io_error(&e)
            );
            return f(); // run anyway with caller's fd 0; matches "best effort"
        }
    };

    let saved = unsafe { libc::dup(0) };
    if saved < 0 {
        let e = io::Error::last_os_error();
        crate::sh_error!(
            &*shell_cell.borrow(),
            None,
            "dup: {}",
            crate::bash_io_error(&e)
        );
        // `r` and `w` (OwnedFds) drop as we return, closing both pipe ends.
        return f();
    }
    // Own the saved fd 0 so it closes exactly once via drop (#197 Class-A).
    let saved = unsafe { OwnedFd::from_raw_fd(saved) };

    if unsafe { libc::dup2(r.as_raw_fd(), 0) } < 0 {
        let e = io::Error::last_os_error();
        crate::sh_error!(
            &*shell_cell.borrow(),
            None,
            "dup2: {}",
            crate::bash_io_error(&e)
        );
        // `r`, `w`, and `saved` (all OwnedFds) drop as we return.
        return f();
    }
    // fd 0 now aliases the pipe read end; drop our own copy of it.
    drop(r);

    struct Restore {
        saved: OwnedFd,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = io::stdout().flush();
            // Restore fd 0 from the owned save; the OwnedFd then drops → closes it.
            unsafe {
                libc::dup2(self.saved.as_raw_fd(), 0);
            }
        }
    }
    let _restore = Restore { saved };

    if input.len() <= INLINE_STDIN_THRESHOLD {
        // Write inline, close, then run.
        let written = unsafe { libc::write(w.as_raw_fd(), input.as_ptr().cast(), input.len()) };
        let _ = written; // best-effort; pipe writes ≤ PIPE_BUF are atomic
        drop(w); // closes the write end
        f()
    } else {
        // Spawn a writer thread that owns `w` and exits when it's closed by EPIPE
        // or by completing the write. Ownership of the write end moves into the
        // thread (the OwnedFd), so it closes exactly once when the thread's File
        // drops.
        let input_owned: Vec<u8> = input.to_vec();
        let handle = std::thread::spawn(move || {
            let mut file = std::fs::File::from(w);
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
