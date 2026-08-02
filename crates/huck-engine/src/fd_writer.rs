//! Unbuffered writer over a real fd, used for builtin stdout.
//!
//! Rust's process-global `io::stdout()` is unusable for this: it SWALLOWS EBADF
//! (`std::io::stdio::handle_ebadf` upstream reports success for a write that
//! genuinely failed), it is a `LineWriter` — so whether an error surfaces at
//! `write` or at a later `flush` depends on a trailing newline — and it RETAINS
//! unwritten bytes after a failed write, which then reach whatever fd 1 is
//! restored to. See #186 / #190 / #191 and the v308 spec.

use crate::capture_test_hook;
use std::io;
use std::os::unix::io::RawFd;

/// Which in-memory capture stream a writer feeds when a `#[cfg(test)]` capture
/// is active on the thread (see `capture_test_hook`). `Out` → the captured
/// stdout buffer; `Err` → the captured stderr buffer (which folds into stdout
/// under `merge_stderr`). Used to honor a trailing `>&2` / `2>&1` on a captured
/// builtin without a real fd redirect. In production no capture is ever active,
/// so this only affects the test capture paths.
#[derive(Clone, Copy)]
pub(crate) enum CaptureStream {
    Out,
    Err,
}

/// An unbuffered writer over a raw fd.
///
/// Every `write` is a direct `write(2)`, so the caller sees the real errno —
/// unlike `io::stdout()`, which swallows EBADF. Nothing is buffered, so a
/// failed write leaves no bytes behind to reach a later, different fd.
///
/// The first errno is recorded so the caller can report it even for the many
/// builtins that discard their own write `Result`.
///
/// `cap_stream` (test capture only): when `Some`, and a thread-local capture is
/// active, bytes are appended to that capture stream instead of hitting the real
/// fd. `None` (the default) always writes the real fd — used when fd 1 is
/// redirected to a real target (`>file`) so the redirect wins over an outer
/// capture, mirroring the deleted `force Terminal` behavior.
pub(crate) struct FdWriter {
    fd: RawFd,
    first_errno: Option<i32>,
    cap_stream: Option<CaptureStream>,
}

impl FdWriter {
    /// A plain real-fd writer (no capture routing). Only the unit tests below
    /// construct one directly; the executor always tags a capture stream via
    /// `with_capture`.
    #[cfg(test)]
    pub(crate) fn new(fd: RawFd) -> Self {
        Self {
            fd,
            first_errno: None,
            cap_stream: None,
        }
    }

    /// The executor's constructor: feeds `cap_stream` when a test capture is
    /// active, else writes the real `fd`.
    pub(crate) fn with_capture(fd: RawFd, cap_stream: Option<CaptureStream>) -> Self {
        Self {
            fd,
            first_errno: None,
            cap_stream,
        }
    }

    /// The first errno this writer saw, if any.
    pub(crate) fn first_error(&self) -> Option<io::Error> {
        self.first_errno.map(io::Error::from_raw_os_error)
    }
}

impl io::Write for FdWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // The zero-byte rule: bash reports a write error only when a write(2)
        // actually failed, and it attempts none for empty output (`echo -n ''
        // >&3` is silent, rc 0). A zero-byte write(2) to a bad fd DOES return
        // EBADF, so we must not issue one.
        if buf.is_empty() {
            return Ok(0);
        }
        // Test capture: if a capture is active and this writer feeds a stream,
        // intercept the bytes before the real fd. Compiles to a `false` no-op in
        // production, so the direct write below is unchanged.
        if let Some(stream) = self.cap_stream {
            let handled = match stream {
                CaptureStream::Out => capture_test_hook::push_out(buf),
                CaptureStream::Err => capture_test_hook::push_err(buf),
            };
            if handled {
                return Ok(buf.len());
            }
        }
        loop {
            let n = unsafe { libc::write(self.fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
            if n < 0 {
                let e = io::Error::last_os_error();
                // EINTR is not a failure — retry (mirrors executor.rs:7160).
                if e.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                if self.first_errno.is_none() {
                    self.first_errno = e.raw_os_error();
                }
                return Err(e);
            }
            // A short count is normal (e.g. a full pipe); `write_all` loops.
            return Ok(n as usize);
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The stderr counterpart writer used at every `err_writer` chokepoint. Real
/// writes go to `io::stderr()` (unbuffered, unchanged from before). When a
/// `#[cfg(test)]` capture is active it routes bytes to the capture stream —
/// `Err` normally, `Out` for a captured builtin's `2>&1` (stderr → stdout). In
/// production `push_*` are `false` no-ops, so this is exactly `io::stderr()`.
pub(crate) struct CaptureStderr {
    /// `Some` → feed this capture stream when a test capture is active; `None` →
    /// always the real fd 2 (used when fd 2 is redirected to a real target, so
    /// the redirect wins over an outer capture).
    stream: Option<CaptureStream>,
}

impl CaptureStderr {
    /// A stderr writer that feeds `stream` under a `#[cfg(test)]` capture.
    pub(crate) fn new(stream: CaptureStream) -> Self {
        Self {
            stream: Some(stream),
        }
    }

    /// A stderr writer whose capture routing is `stream` (or `None` = real fd 2
    /// only, ignoring any active capture).
    pub(crate) fn with_capture(stream: Option<CaptureStream>) -> Self {
        Self { stream }
    }
}

impl io::Write for CaptureStderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !buf.is_empty() {
            if let Some(stream) = self.stream {
                let handled = match stream {
                    CaptureStream::Out => capture_test_hook::push_out(buf),
                    CaptureStream::Err => capture_test_hook::push_err(buf),
                };
                if handled {
                    return Ok(buf.len());
                }
            }
        }
        io::Write::write(&mut io::stderr(), buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::Write::flush(&mut io::stderr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Open a read-only fd (writes to it fail with EBADF).
    ///
    /// `/dev/null` rather than a real file: POSIX makes `write(2)` fail EBADF
    /// for ANY fd not open for writing, so the underlying file is irrelevant —
    /// and `/dev/null` is the one path guaranteed to exist on every unix.
    /// (This used to be `/etc/hostname`, which macOS does not have — #297.)
    fn ro_fd() -> RawFd {
        let p = c"/dev/null";
        let fd = unsafe { libc::open(p.as_ptr(), libc::O_RDONLY) };
        assert!(fd >= 0, "open /dev/null O_RDONLY failed");
        fd
    }

    /// Open /dev/full (writes to it fail with ENOSPC). Linux-only — no other
    /// unix ships a device that reports ENOSPC on demand (#297).
    #[cfg(target_os = "linux")]
    fn full_fd() -> RawFd {
        let p = c"/dev/full";
        let fd = unsafe { libc::open(p.as_ptr(), libc::O_WRONLY) };
        assert!(fd >= 0, "open /dev/full failed");
        fd
    }

    /// The write end of a pipe whose read end is already closed: writes to it
    /// fail EPIPE. The portable stand-in for `/dev/full` as a "the errno is
    /// surfaced verbatim, and it isn't EBADF" fixture.
    ///
    /// Rust's runtime sets SIGPIPE to SIG_IGN for the test process, so the
    /// failing write returns rather than killing the harness; this asserts that
    /// rather than assuming it, so a runtime change fails loudly instead of
    /// silently aborting the suite.
    fn broken_pipe_fd() -> RawFd {
        assert_eq!(
            unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) },
            libc::SIG_IGN,
            "test harness must have SIGPIPE ignored for an EPIPE write to return"
        );
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { libc::close(fds[0]) };
        fds[1]
    }

    #[test]
    fn write_to_read_only_fd_surfaces_ebadf() {
        let fd = ro_fd();
        let mut w = FdWriter::new(fd);
        let e = w
            .write_all(b"x")
            .expect_err("write to a read-only fd must fail");
        assert_eq!(e.raw_os_error(), Some(libc::EBADF));
        unsafe { libc::close(fd) };
    }

    #[test]
    fn write_to_closed_fd_surfaces_ebadf() {
        let fd = ro_fd();
        unsafe { libc::close(fd) };
        let mut w = FdWriter::new(fd);
        let e = w
            .write_all(b"x")
            .expect_err("write to a closed fd must fail");
        assert_eq!(e.raw_os_error(), Some(libc::EBADF));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn write_to_dev_full_surfaces_enospc() {
        let fd = full_fd();
        let mut w = FdWriter::new(fd);
        let e = w.write_all(b"x").expect_err("write to /dev/full must fail");
        assert_eq!(e.raw_os_error(), Some(libc::ENOSPC));
        unsafe { libc::close(fd) };
    }

    /// The portable half of the above: a non-EBADF errno must reach the caller
    /// unchanged. On Linux this runs ALONGSIDE the /dev/full ENOSPC check; on
    /// platforms with no ENOSPC-on-demand device it is the only cover for
    /// "the writer does not collapse every failure into EBADF".
    #[test]
    fn write_to_broken_pipe_surfaces_epipe() {
        let fd = broken_pipe_fd();
        let mut w = FdWriter::new(fd);
        let e = w
            .write_all(b"x")
            .expect_err("write to a broken pipe must fail");
        assert_eq!(e.raw_os_error(), Some(libc::EPIPE));
        unsafe { libc::close(fd) };
    }

    /// THE ZERO-BYTE RULE. A zero-byte write(2) to a bad fd returns -1/EBADF
    /// (measured), but bash is SILENT for `echo -n '' >&3` (rc 0) because it
    /// never attempts a write. So an empty write must perform NO syscall.
    /// If the short-circuit is dropped, this test fails.
    #[test]
    fn empty_write_performs_no_syscall_on_a_bad_fd() {
        let fd = ro_fd();
        let mut w = FdWriter::new(fd);
        assert_eq!(w.write(b"").expect("empty write must succeed"), 0);
        w.write_all(b"").expect("empty write_all must succeed");
        assert!(
            w.first_error().is_none(),
            "an empty write must not record an error"
        );
        unsafe { libc::close(fd) };
    }

    #[test]
    fn records_first_error_only() {
        let fd = ro_fd();
        let mut w = FdWriter::new(fd);
        let _ = w.write_all(b"x");
        let _ = w.write_all(b"y");
        let e = w.first_error().expect("an error must be recorded");
        assert_eq!(e.raw_os_error(), Some(libc::EBADF));
        unsafe { libc::close(fd) };
    }

    #[test]
    fn no_error_recorded_on_success() {
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let mut w = FdWriter::new(fds[1]);
        w.write_all(b"hello").expect("write to a pipe must succeed");
        assert!(w.first_error().is_none());
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }
    }

    /// A large payload exceeds the pipe capacity in one write(2), so the kernel
    /// returns a PARTIAL count. `write_all` must loop until every byte lands.
    #[test]
    fn partial_writes_complete_via_write_all() {
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (r, wfd) = (fds[0], fds[1]);
        // 256KB > the 64KB default pipe capacity: the writer must block/loop.
        let payload = vec![b'z'; 256 * 1024];
        let reader = std::thread::spawn(move || {
            let mut got = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                let n = unsafe { libc::read(r, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n <= 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n as usize]);
            }
            unsafe { libc::close(r) };
            got.len()
        });
        let mut w = FdWriter::new(wfd);
        w.write_all(&payload).expect("write_all must complete");
        assert!(w.first_error().is_none());
        unsafe { libc::close(wfd) };
        assert_eq!(reader.join().expect("reader thread"), payload.len());
    }

    #[test]
    fn flush_is_a_noop_and_succeeds() {
        let fd = ro_fd();
        let mut w = FdWriter::new(fd);
        w.flush().expect("flush must be a no-op that succeeds");
        unsafe { libc::close(fd) };
    }
}
