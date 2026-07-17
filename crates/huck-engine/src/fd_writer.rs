//! Unbuffered writer over a real fd, used for builtin stdout.
//!
//! Rust's process-global `io::stdout()` is unusable for this: it SWALLOWS EBADF
//! (`std::io::stdio::handle_ebadf` upstream reports success for a write that
//! genuinely failed), it is a `LineWriter` — so whether an error surfaces at
//! `write` or at a later `flush` depends on a trailing newline — and it RETAINS
//! unwritten bytes after a failed write, which then reach whatever fd 1 is
//! restored to. See #186 / #190 / #191 and the v308 spec.

use std::io;
use std::os::unix::io::RawFd;

/// An unbuffered writer over a raw fd.
///
/// Every `write` is a direct `write(2)`, so the caller sees the real errno —
/// unlike `io::stdout()`, which swallows EBADF. Nothing is buffered, so a
/// failed write leaves no bytes behind to reach a later, different fd.
///
/// The first errno is recorded so the caller can report it even for the many
/// builtins that discard their own write `Result`.
pub(crate) struct FdWriter {
    fd: RawFd,
    first_errno: Option<i32>,
}

impl FdWriter {
    pub(crate) fn new(fd: RawFd) -> Self {
        Self {
            fd,
            first_errno: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Open a read-only fd (writes to it fail with EBADF).
    fn ro_fd() -> RawFd {
        let p = c"/etc/hostname";
        let fd = unsafe { libc::open(p.as_ptr(), libc::O_RDONLY) };
        assert!(fd >= 0, "open /etc/hostname failed");
        fd
    }

    /// Open /dev/full (writes to it fail with ENOSPC).
    fn full_fd() -> RawFd {
        let p = c"/dev/full";
        let fd = unsafe { libc::open(p.as_ptr(), libc::O_WRONLY) };
        assert!(fd >= 0, "open /dev/full failed");
        fd
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

    #[test]
    fn write_to_dev_full_surfaces_enospc() {
        let fd = full_fd();
        let mut w = FdWriter::new(fd);
        let e = w.write_all(b"x").expect_err("write to /dev/full must fail");
        assert_eq!(e.raw_os_error(), Some(libc::ENOSPC));
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
