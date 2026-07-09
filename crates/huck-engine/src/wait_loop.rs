//! Cross-platform poller: wait on N pipe file descriptors plus a SIGCHLD
//! event source, returning ready events. Replaces blocking `waitpid` in
//! the external-process capture path so the embedder's thread can
//! dispatch streaming callbacks in real time.
//!
//! Linux: signalfd(SIGCHLD) + poll(2).
//! macOS: kqueue with EVFILT_SIGNAL(SIGCHLD) + EVFILT_READ on pipes.

use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Event {
    Readable(RawFd),
    ChildExited,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    #[allow(dead_code)]
    pub struct WaitLoop {
        sigchld_fd: Option<RawFd>,
        pipes: Vec<RawFd>,
        // Old signal mask so Drop can restore.
        saved_mask: Option<libc::sigset_t>,
    }

    #[allow(dead_code)]
    impl WaitLoop {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                sigchld_fd: None,
                pipes: Vec::new(),
                saved_mask: None,
            })
        }

        pub fn register_pipe(&mut self, fd: RawFd) -> io::Result<()> {
            self.pipes.push(fd);
            Ok(())
        }

        /// Stop polling `fd`. The fd itself is NOT closed; caller manages
        /// lifetime. Useful when a pipe has reached EOF and would otherwise
        /// cause poll to busy-spin on the latched POLLHUP.
        pub fn unregister_pipe(&mut self, fd: RawFd) {
            self.pipes.retain(|&f| f != fd);
        }

        pub fn register_sigchld(&mut self) -> io::Result<()> {
            // signalfd requires SIGCHLD blocked. Block on the calling thread.
            // SAFETY: zero-init sigset_t then sigaddset is standard libc usage.
            let mut new_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe { libc::sigemptyset(&mut new_mask) };
            unsafe { libc::sigaddset(&mut new_mask, libc::SIGCHLD) };

            let mut old_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            let ret = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &new_mask, &mut old_mask) };
            if ret != 0 {
                return Err(io::Error::from_raw_os_error(ret));
            }
            self.saved_mask = Some(old_mask);

            let fd =
                unsafe { libc::signalfd(-1, &new_mask, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            self.sigchld_fd = Some(fd);
            Ok(())
        }

        pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<Vec<Event>> {
            let timeout_ms: i32 = match timeout {
                None => -1,
                Some(d) => {
                    let ms = d.as_millis();
                    if ms > i32::MAX as u128 {
                        i32::MAX
                    } else {
                        ms as i32
                    }
                }
            };
            let mut pollfds: Vec<libc::pollfd> = self
                .pipes
                .iter()
                .map(|&fd| libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            if let Some(fd) = self.sigchld_fd {
                pollfds.push(libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            let n = unsafe {
                libc::poll(
                    pollfds.as_mut_ptr(),
                    pollfds.len() as libc::nfds_t,
                    timeout_ms,
                )
            };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    return Ok(Vec::new());
                }
                return Err(err);
            }
            let mut events = Vec::new();
            for pfd in &pollfds {
                if pfd.revents == 0 {
                    continue;
                }
                if Some(pfd.fd) == self.sigchld_fd {
                    // Drain the signalfd so it returns to non-ready.
                    let mut buf = [0u8; std::mem::size_of::<libc::signalfd_siginfo>() * 4];
                    let _ = unsafe { libc::read(pfd.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
                    events.push(Event::ChildExited);
                } else if pfd.revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                    events.push(Event::Readable(pfd.fd));
                }
            }
            Ok(events)
        }
    }

    impl Drop for WaitLoop {
        fn drop(&mut self) {
            if let Some(fd) = self.sigchld_fd.take() {
                unsafe { libc::close(fd) };
            }
            if let Some(mask) = self.saved_mask.take() {
                let _ = unsafe {
                    libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut())
                };
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub struct WaitLoop {
        kq: RawFd,
        #[allow(dead_code)]
        pipes: Vec<RawFd>,
        #[allow(dead_code)]
        sigchld_registered: bool,
        saved_mask: Option<libc::sigset_t>,
    }

    impl WaitLoop {
        pub fn new() -> io::Result<Self> {
            let kq = unsafe { libc::kqueue() };
            if kq < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                kq,
                pipes: Vec::new(),
                sigchld_registered: false,
                saved_mask: None,
            })
        }

        pub fn register_pipe(&mut self, fd: RawFd) -> io::Result<()> {
            let kev = libc::kevent {
                ident: fd as usize,
                filter: libc::EVFILT_READ,
                flags: libc::EV_ADD | libc::EV_ENABLE,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let ret = unsafe {
                libc::kevent(self.kq, &kev, 1, std::ptr::null_mut(), 0, std::ptr::null())
            };
            if ret < 0 {
                return Err(io::Error::last_os_error());
            }
            self.pipes.push(fd);
            Ok(())
        }

        /// Stop polling `fd`. The fd itself is NOT closed; caller manages
        /// lifetime. Mirrors the Linux unregister: kqueue's EV_DELETE removes
        /// the registration so we don't loop on the latched EOF.
        pub fn unregister_pipe(&mut self, fd: RawFd) {
            let kev = libc::kevent {
                ident: fd as usize,
                filter: libc::EVFILT_READ,
                flags: libc::EV_DELETE,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let _ = unsafe {
                libc::kevent(self.kq, &kev, 1, std::ptr::null_mut(), 0, std::ptr::null())
            };
            self.pipes.retain(|&f| f != fd);
        }

        pub fn register_sigchld(&mut self) -> io::Result<()> {
            // Block SIGCHLD so default handler doesn't preempt the kqueue event.
            let mut new_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe { libc::sigemptyset(&mut new_mask) };
            unsafe { libc::sigaddset(&mut new_mask, libc::SIGCHLD) };
            let mut old_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            let ret = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &new_mask, &mut old_mask) };
            if ret != 0 {
                return Err(io::Error::from_raw_os_error(ret));
            }
            self.saved_mask = Some(old_mask);

            let kev = libc::kevent {
                ident: libc::SIGCHLD as usize,
                filter: libc::EVFILT_SIGNAL,
                flags: libc::EV_ADD | libc::EV_ENABLE,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let ret = unsafe {
                libc::kevent(self.kq, &kev, 1, std::ptr::null_mut(), 0, std::ptr::null())
            };
            if ret < 0 {
                return Err(io::Error::last_os_error());
            }
            self.sigchld_registered = true;
            Ok(())
        }

        pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<Vec<Event>> {
            let ts_spec;
            let ts_ptr: *const libc::timespec = match timeout {
                None => std::ptr::null(),
                Some(d) => {
                    ts_spec = libc::timespec {
                        tv_sec: d.as_secs() as libc::time_t,
                        tv_nsec: d.subsec_nanos() as libc::c_long,
                    };
                    &ts_spec
                }
            };
            let mut out: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
            let n = unsafe {
                libc::kevent(
                    self.kq,
                    std::ptr::null(),
                    0,
                    out.as_mut_ptr(),
                    out.len() as i32,
                    ts_ptr,
                )
            };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    return Ok(Vec::new());
                }
                return Err(err);
            }
            let mut events = Vec::new();
            for kev in &out[..n as usize] {
                if kev.filter == libc::EVFILT_SIGNAL && kev.ident as i32 == libc::SIGCHLD {
                    events.push(Event::ChildExited);
                } else if kev.filter == libc::EVFILT_READ {
                    events.push(Event::Readable(kev.ident as RawFd));
                }
            }
            Ok(events)
        }
    }

    impl Drop for WaitLoop {
        fn drop(&mut self) {
            unsafe { libc::close(self.kq) };
            if let Some(mask) = self.saved_mask.take() {
                let _ = unsafe {
                    libc::pthread_sigmask(libc::SIG_SETMASK, &mask, std::ptr::null_mut())
                };
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
pub use linux::WaitLoop;
#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use macos::WaitLoop;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("huck-engine WaitLoop requires target_os linux or macos");

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::RawFd;
    use std::time::{Duration, Instant};

    fn make_pipe() -> (RawFd, RawFd) {
        let mut fds = [0; 2];
        let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        assert_eq!(ret, 0);
        (fds[0], fds[1])
    }

    #[test]
    fn poll_returns_when_pipe_becomes_readable() {
        let mut wl = WaitLoop::new().unwrap();
        let (r, w) = make_pipe();
        wl.register_pipe(r).unwrap();
        // Write before polling — pipe is already readable.
        unsafe { libc::write(w, b"hi\n".as_ptr() as *const _, 3) };
        let evs = wl.poll(Some(Duration::from_millis(100))).unwrap();
        assert!(evs.contains(&Event::Readable(r)));
        unsafe {
            libc::close(r);
            libc::close(w);
        }
    }

    #[test]
    fn poll_returns_empty_on_timeout() {
        let mut wl = WaitLoop::new().unwrap();
        let (r, w) = make_pipe();
        wl.register_pipe(r).unwrap();
        let start = Instant::now();
        let evs = wl.poll(Some(Duration::from_millis(50))).unwrap();
        let elapsed = start.elapsed();
        assert!(evs.is_empty(), "expected no events on timeout, got {evs:?}");
        assert!(
            elapsed >= Duration::from_millis(40),
            "elapsed too short: {elapsed:?}"
        );
        unsafe {
            libc::close(r);
            libc::close(w);
        }
    }

    #[test]
    fn poll_returns_child_exited_when_sigchld_fires() {
        let mut wl = WaitLoop::new().unwrap();
        wl.register_sigchld().unwrap();
        // Fork a child that exits immediately.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            // Child: exit.
            unsafe { libc::_exit(0) };
        }
        // Parent: redirect SIGCHLD to this thread. In a single-threaded
        // program the kernel would deliver the child-exit SIGCHLD here
        // automatically (since SIGCHLD is blocked on this thread, it stays
        // pending, signalfd reads it). Under cargo's test runtime the
        // process has other threads with SIGCHLD unblocked, so the kernel
        // may deliver there and its default disposition (ignore) consumes
        // the signal before signalfd sees it. `raise(SIGCHLD)` directs the
        // signal at this thread's pending set on Linux/macOS, matching the
        // production scenario where the embedder's WaitLoop thread is the
        // signal target.
        unsafe { libc::raise(libc::SIGCHLD) };
        let evs = wl.poll(Some(Duration::from_secs(2))).unwrap();
        assert!(
            evs.contains(&Event::ChildExited),
            "expected ChildExited, got {evs:?}"
        );
        // Reap the child.
        let mut status = 0;
        unsafe { libc::waitpid(pid, &mut status, 0) };
    }

    #[test]
    fn drop_restores_signal_mask() {
        let prev_mask: libc::sigset_t = unsafe {
            let mut m = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut m);
            m
        };
        {
            let mut wl = WaitLoop::new().unwrap();
            wl.register_sigchld().unwrap();
            // SIGCHLD should be blocked now.
            let blocked: libc::sigset_t = unsafe {
                let mut m = std::mem::zeroed();
                libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut m);
                m
            };
            assert_eq!(unsafe { libc::sigismember(&blocked, libc::SIGCHLD) }, 1);
        }
        // After Drop: SIGCHLD mask should be back to its prior state.
        let after: libc::sigset_t = unsafe {
            let mut m = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_BLOCK, std::ptr::null(), &mut m);
            m
        };
        assert_eq!(
            unsafe { libc::sigismember(&after, libc::SIGCHLD) },
            unsafe { libc::sigismember(&prev_mask, libc::SIGCHLD) },
        );
    }
}
