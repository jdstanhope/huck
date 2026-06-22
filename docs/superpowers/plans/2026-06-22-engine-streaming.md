# v207 `Engine::exec` streaming-output callbacks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `.on_stdout_line(cb)` and `.on_stderr_line(cb)` to `ExecBuilder`, firing real-time line callbacks on the embedder's thread for both builtin and external output.

**Architecture:** Two new helper modules (`line_buf.rs` for partial-line accumulation, `wait_loop.rs` for cross-platform pipe+SIGCHLD polling). Builder gains two `Option<Box<dyn FnMut(&str) + 'a>>` fields plus methods. Builtin write site (`StdoutSink::Capture`) gains an in-process line-dispatch hook. External-process paths (3 fork sites) replace blocking `waitpid` + drainer-thread with a poll loop driven by `WaitLoop` on the embedder's thread — real-time, no internal threads, no `Send` bound.

**Tech Stack:** Rust 2021, `libc` for `pipe2`/`signalfd`/`kqueue`/`poll`/`waitpid`. Linux + macOS only (Windows is a compile-error). No new crate deps.

**Branch:** `v207-engine-streaming`. Each task ends with a green-suite commit.

**Spec:** `docs/superpowers/specs/2026-06-22-engine-streaming-design.md`.

---

## File structure

**Create:**
- `crates/huck-engine/src/line_buf.rs` — `LineBuf` accumulator (push bytes, pull `Option<String>` lines, drain final partial). ~50 LOC + tests.
- `crates/huck-engine/src/wait_loop.rs` — cross-platform `WaitLoop { register_pipe, register_sigchld, poll }`. Two platform impls behind `#[cfg(target_os = …)]`. ~200 LOC + tests.
- `crates/huck-engine/src/stream_loop.rs` — the shared `external_capture_loop` helper that the 3 fork sites delegate to. ~150 LOC.
- `crates/huck-engine/examples/engine_stream_diff.rs` — self-consistency driver. ~40 LOC.
- `tests/scripts/engine_stream_consistency_check.sh` — harness.

**Modify:**
- `crates/huck-engine/src/exec_builder.rs` — 2 fields + 2 methods + thread `Callbacks` through `run_with_sinks`.
- `crates/huck-engine/src/executor.rs` — builtin-path dispatch hook at the `StdoutSink::Capture` write site; replace blocking external-wait at 3 fork sites with `external_capture_loop`.
- `crates/huck-engine/src/engine.rs` — append streaming unit tests (~30 tests) + doctest update.
- `crates/huck-engine/src/lib.rs` — declare the 3 new modules.
- `docs/architecture.md` — paragraph on the streaming API.

---

## Task 1: `LineBuf` accumulator

**Files:**
- Create: `crates/huck-engine/src/line_buf.rs`
- Modify: `crates/huck-engine/src/lib.rs` (declare `pub(crate) mod line_buf;`)

- [ ] **Step 1: Create the branch**

```bash
git checkout -b v207-engine-streaming
```

- [ ] **Step 2: Create the module**

```rust
// crates/huck-engine/src/line_buf.rs
//! Accumulate raw byte chunks, dispatch complete (newline-terminated) lines.
//!
//! Used by both the builtin-path dispatch hook (where bytes are written into
//! a Vec<u8> capture buffer) and the external poll loop (where bytes are
//! `read(2)` from a pipe).

#[derive(Default)]
pub struct LineBuf {
    partial: Vec<u8>,
}

impl LineBuf {
    pub fn new() -> Self {
        Self { partial: Vec::new() }
    }

    /// Append raw bytes. Caller pulls via `next_line()` after each push.
    pub fn push(&mut self, bytes: &[u8]) {
        self.partial.extend_from_slice(bytes);
    }

    /// Pull the next complete line (without trailing `\n`). Returns `None`
    /// when no more `\n` is present in the buffer.
    ///
    /// Decodes via `String::from_utf8_lossy` — invalid UTF-8 becomes U+FFFD,
    /// matching `Output.stdout` policy.
    pub fn next_line(&mut self) -> Option<String> {
        let pos = self.partial.iter().position(|&b| b == b'\n')?;
        let line_bytes: Vec<u8> = self.partial.drain(..=pos).collect();
        // line_bytes ends in \n; trim it.
        let trimmed = &line_bytes[..line_bytes.len() - 1];
        Some(String::from_utf8_lossy(trimmed).into_owned())
    }

    /// Pull whatever bytes remain (may be empty). For end-of-stream flush.
    /// Returns `None` if the buffer is empty (no final partial to deliver).
    pub fn drain_final(&mut self) -> Option<String> {
        if self.partial.is_empty() {
            return None;
        }
        let rest = std::mem::take(&mut self.partial);
        Some(String::from_utf8_lossy(&rest).into_owned())
    }

    /// Is the partial buffer empty? Used by debugging assertions.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.partial.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_complete_line() {
        let mut b = LineBuf::new();
        b.push(b"hello\n");
        assert_eq!(b.next_line().as_deref(), Some("hello"));
        assert_eq!(b.next_line(), None);
        assert!(b.is_empty());
    }

    #[test]
    fn multiple_lines_in_one_push() {
        let mut b = LineBuf::new();
        b.push(b"a\nb\nc\n");
        assert_eq!(b.next_line().as_deref(), Some("a"));
        assert_eq!(b.next_line().as_deref(), Some("b"));
        assert_eq!(b.next_line().as_deref(), Some("c"));
        assert_eq!(b.next_line(), None);
    }

    #[test]
    fn split_line_across_pushes() {
        let mut b = LineBuf::new();
        b.push(b"hel");
        assert_eq!(b.next_line(), None);
        b.push(b"lo\n");
        assert_eq!(b.next_line().as_deref(), Some("hello"));
    }

    #[test]
    fn empty_line() {
        let mut b = LineBuf::new();
        b.push(b"\n");
        assert_eq!(b.next_line().as_deref(), Some(""));
        assert_eq!(b.next_line(), None);
    }

    #[test]
    fn drain_final_partial() {
        let mut b = LineBuf::new();
        b.push(b"trailing");
        assert_eq!(b.next_line(), None);
        assert_eq!(b.drain_final().as_deref(), Some("trailing"));
        assert_eq!(b.drain_final(), None);
    }

    #[test]
    fn drain_final_after_complete_line_is_empty() {
        let mut b = LineBuf::new();
        b.push(b"hi\n");
        let _ = b.next_line();
        assert_eq!(b.drain_final(), None);
    }

    #[test]
    fn invalid_utf8_decoded_lossy() {
        let mut b = LineBuf::new();
        b.push(&[0xff, 0xfe, b'\n']);
        let line = b.next_line().unwrap();
        // U+FFFD is the replacement character, 3 bytes in UTF-8.
        assert!(line.contains('\u{FFFD}'));
    }
}
```

- [ ] **Step 3: Register module in lib.rs**

In `crates/huck-engine/src/lib.rs`, add a new line near the other v206 modules:

```rust
pub(crate) mod line_buf;
```

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet line_buf
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 7 `line_buf::tests::*` pass; full suite green; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/line_buf.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v207 task 1: LineBuf accumulator

Pure-stdlib helper for accumulating raw byte chunks and dispatching
newline-terminated lines. push(bytes) appends; next_line() pulls one complete
line as String (UTF-8-lossy decode, trailing \n stripped); drain_final()
flushes any remaining partial at EOF. Used by both the builtin-path dispatch
hook (task 4) and the external poll loop (task 5).

7 unit tests cover single line, multi-line-per-push, split-across-pushes,
empty line, drain-final partial, drain-final after complete-line is empty,
and invalid-UTF8 lossy decode.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `WaitLoop` cross-platform pipe + SIGCHLD poller

**Files:**
- Create: `crates/huck-engine/src/wait_loop.rs`
- Modify: `crates/huck-engine/src/lib.rs` (declare `pub(crate) mod wait_loop;`)

- [ ] **Step 1: Create the module**

```rust
// crates/huck-engine/src/wait_loop.rs
//! Cross-platform poller: wait on N pipe file descriptors plus a SIGCHLD
//! event source, returning ready events. Replaces blocking `waitpid` in
//! the external-process capture path (v207) so the embedder's thread can
//! dispatch streaming callbacks in real time.
//!
//! Linux: signalfd(SIGCHLD) + poll(2).
//! macOS: kqueue with EVFILT_SIGNAL(SIGCHLD) + EVFILT_READ on pipes.

use std::io;
use std::os::fd::RawFd;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Event {
    Readable(RawFd),
    ChildExited,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    pub struct WaitLoop {
        sigchld_fd: Option<RawFd>,
        pipes: Vec<RawFd>,
        // Old signal mask so Drop can restore.
        saved_mask: Option<libc::sigset_t>,
    }

    impl WaitLoop {
        pub fn new() -> io::Result<Self> {
            Ok(Self { sigchld_fd: None, pipes: Vec::new(), saved_mask: None })
        }

        pub fn register_pipe(&mut self, fd: RawFd) -> io::Result<()> {
            self.pipes.push(fd);
            Ok(())
        }

        pub fn register_sigchld(&mut self) -> io::Result<()> {
            // signalfd requires SIGCHLD blocked. Block on the calling thread.
            // SAFETY: zero-init sigset_t then sigaddset is standard libc usage.
            let mut new_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe { libc::sigemptyset(&mut new_mask) };
            unsafe { libc::sigaddset(&mut new_mask, libc::SIGCHLD) };

            let mut old_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            let ret = unsafe {
                libc::pthread_sigmask(libc::SIG_BLOCK, &new_mask, &mut old_mask)
            };
            if ret != 0 {
                return Err(io::Error::from_raw_os_error(ret));
            }
            self.saved_mask = Some(old_mask);

            let fd = unsafe {
                libc::signalfd(-1, &new_mask, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK)
            };
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
                    if ms > i32::MAX as u128 { i32::MAX } else { ms as i32 }
                }
            };
            let mut pollfds: Vec<libc::pollfd> = self
                .pipes
                .iter()
                .map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 })
                .collect();
            if let Some(fd) = self.sigchld_fd {
                pollfds.push(libc::pollfd { fd, events: libc::POLLIN, revents: 0 });
            }
            let n = unsafe {
                libc::poll(pollfds.as_mut_ptr(), pollfds.len() as libc::nfds_t, timeout_ms)
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
                    let _ = unsafe {
                        libc::read(pfd.fd, buf.as_mut_ptr() as *mut _, buf.len())
                    };
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
        pipes: Vec<RawFd>,
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

        pub fn register_sigchld(&mut self) -> io::Result<()> {
            // Block SIGCHLD so default handler doesn't preempt the kqueue event.
            let mut new_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe { libc::sigemptyset(&mut new_mask) };
            unsafe { libc::sigaddset(&mut new_mask, libc::SIGCHLD) };
            let mut old_mask: libc::sigset_t = unsafe { std::mem::zeroed() };
            let ret = unsafe {
                libc::pthread_sigmask(libc::SIG_BLOCK, &new_mask, &mut old_mask)
            };
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
pub use linux::WaitLoop;
#[cfg(target_os = "macos")]
pub use macos::WaitLoop;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("huck-engine v207 WaitLoop requires target_os linux or macos");

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
        unsafe { libc::close(r); libc::close(w); }
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
        assert!(elapsed >= Duration::from_millis(40), "elapsed too short: {elapsed:?}");
        unsafe { libc::close(r); libc::close(w); }
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
        // Parent: poll for SIGCHLD.
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
            assert_eq!(
                unsafe { libc::sigismember(&blocked, libc::SIGCHLD) },
                1
            );
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
```

- [ ] **Step 2: Register module in lib.rs**

Add `pub(crate) mod wait_loop;` to `crates/huck-engine/src/lib.rs` near the other v207 modules.

- [ ] **Step 3: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet wait_loop
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 4 `wait_loop::tests::*` pass on Linux or macOS; full suite green; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/wait_loop.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v207 task 2: WaitLoop cross-platform pipe + SIGCHLD poller

Wait on N pipe file descriptors plus a SIGCHLD event source. Linux impl uses
signalfd(SIGCHLD) + poll(2); macOS impl uses kqueue with EVFILT_SIGNAL +
EVFILT_READ. Both block SIGCHLD via pthread_sigmask (RAII restore on Drop)
so the default handler doesn't preempt our event source. Public API:
WaitLoop::new + register_pipe + register_sigchld + poll(timeout). Other
targets get compile_error!. 4 unit tests cover pipe-readable, timeout-empty,
SIGCHLD-via-fork-and-exit, and Drop-restores-signal-mask.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `ExecBuilder` callback fields + methods

**Files:**
- Modify: `crates/huck-engine/src/exec_builder.rs`

- [ ] **Step 1: Add fields + methods**

In `crates/huck-engine/src/exec_builder.rs`, add to `pub struct ExecBuilder<'a>`:

```rust
pub struct ExecBuilder<'a> {
    // ... existing fields ...
    on_stdout_line: Option<Box<dyn FnMut(&str) + 'a>>,
    on_stderr_line: Option<Box<dyn FnMut(&str) + 'a>>,
}
```

Update `ExecBuilder::new(...)`:

```rust
pub(crate) fn new(engine: &'a mut Engine, src: String) -> Self {
    ExecBuilder {
        // ... existing fields default-initialized ...
        on_stdout_line: None,
        on_stderr_line: None,
    }
}
```

Add to `impl<'a> ExecBuilder<'a>`:

```rust
/// Invoke `f(line)` for each complete line written to stdout. Trailing
/// `\n` stripped. Final partial line (if no trailing newline at EOF) fires
/// once at stream close. Callback runs on the caller's thread.
pub fn on_stdout_line<F: FnMut(&str) + 'a>(mut self, f: F) -> Self {
    self.on_stdout_line = Some(Box::new(f));
    self
}

/// Same for stderr. Under `.merge_stderr()`, stderr is dup2'd onto stdout
/// at the fd level — this callback never fires; all output flows through
/// `on_stdout_line`.
pub fn on_stderr_line<F: FnMut(&str) + 'a>(mut self, f: F) -> Self {
    self.on_stderr_line = Some(Box::new(f));
    self
}
```

- [ ] **Step 2: Build (no behavior change — fields not yet consumed)**

```bash
cargo build --workspace -q
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --quiet
```

Expected: clean build; clippy clean; suite green. The new fields trigger no `unused` lints because they're accessed via `self.on_stdout_line = ...` in the methods. If clippy complains about a dead field, that's expected — Task 4 wires the consumer.

If clippy DOES complain (`field is never read`), add `#[allow(dead_code)]` on the fields with a comment `// consumed by tasks 4+5`. Task 4 will remove the allow.

- [ ] **Step 3: Commit**

```bash
git add crates/huck-engine/src/exec_builder.rs
git commit -m "$(cat <<'EOF'
v207 task 3: ExecBuilder gains on_stdout_line / on_stderr_line fields + methods

Two new builder methods consuming Self: on_stdout_line<F: FnMut(&str) + 'a>
and on_stderr_line. Stored as Option<Box<dyn FnMut(&str) + 'a>> so the
builder's lifetime 'a ties to the closure's borrowed captures. No consumer
yet — tasks 4 and 5 wire the dispatch hooks for builtin and external paths.
No behavior change.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Builtin-path line dispatch hook

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (the `StdoutSink::Capture` and `StderrSink::Capture` write paths)
- Modify: `crates/huck-engine/src/exec_builder.rs` (thread callbacks into `run_with_sinks`)
- Modify: `crates/huck-engine/src/engine.rs` (add builtin-path tests)

For Task 4, we add a `Callbacks { stdout, stderr }` struct that the builder builds from its fields and threads through the run path. The executor's existing `StdoutSink::Capture(buf)` write site gets an "AND ALSO push into LineBuf + dispatch" hook. This task covers BUILTIN-emitted lines only; Task 5 adds the external poll loop.

- [ ] **Step 1: Add a `Callbacks` carrier type**

In `crates/huck-engine/src/exec_builder.rs`, add near the top (after the `use` statements):

```rust
/// Streaming callbacks owned by the builder for the call's duration.
/// `'cb` is the builder's lifetime — closures may borrow caller state for
/// that duration.
pub(crate) struct Callbacks<'cb> {
    pub stdout: Option<Box<dyn FnMut(&str) + 'cb>>,
    pub stderr: Option<Box<dyn FnMut(&str) + 'cb>>,
    pub line_buf_out: crate::line_buf::LineBuf,
    pub line_buf_err: crate::line_buf::LineBuf,
}

impl<'cb> Callbacks<'cb> {
    pub fn new(
        stdout: Option<Box<dyn FnMut(&str) + 'cb>>,
        stderr: Option<Box<dyn FnMut(&str) + 'cb>>,
    ) -> Self {
        Self {
            stdout,
            stderr,
            line_buf_out: crate::line_buf::LineBuf::new(),
            line_buf_err: crate::line_buf::LineBuf::new(),
        }
    }

    pub fn any_set(&self) -> bool {
        self.stdout.is_some() || self.stderr.is_some()
    }

    /// Push raw stdout bytes; dispatch any complete lines via the stdout callback.
    pub fn push_stdout(&mut self, bytes: &[u8]) {
        if self.stdout.is_none() {
            return;
        }
        self.line_buf_out.push(bytes);
        while let Some(line) = self.line_buf_out.next_line() {
            if let Some(cb) = &mut self.stdout {
                cb(&line);
            }
        }
    }

    /// Push raw stderr bytes; dispatch any complete lines via the stderr callback.
    pub fn push_stderr(&mut self, bytes: &[u8]) {
        if self.stderr.is_none() {
            return;
        }
        self.line_buf_err.push(bytes);
        while let Some(line) = self.line_buf_err.next_line() {
            if let Some(cb) = &mut self.stderr {
                cb(&line);
            }
        }
    }

    /// Flush partial-at-EOF lines for both streams.
    pub fn flush_partials(&mut self) {
        if let (Some(line), Some(cb)) = (self.line_buf_out.drain_final(), self.stdout.as_mut()) {
            cb(&line);
        }
        if let (Some(line), Some(cb)) = (self.line_buf_err.drain_final(), self.stderr.as_mut()) {
            cb(&line);
        }
    }
}
```

- [ ] **Step 2: Thread `&mut Callbacks` through `run_program_in_sinks`**

In `crates/huck-engine/src/shell.rs`, find `run_program_in_sinks` and add a new generalization:

```rust
// New function — add it next to run_program_in_sinks.
pub fn run_program_in_sinks_with_callbacks(
    contents: &str,
    argv0: Option<String>,
    args: Vec<String>,
    label: &str,
    push_main_frame: bool,
    sink: &mut crate::executor::StdoutSink,
    err_sink: &mut crate::executor::StderrSink,
    callbacks: Option<&mut crate::exec_builder::Callbacks<'_>>,
    shell_cell: &Rc<RefCell<Shell>>,
) -> i32 {
    // Stash callbacks in a thread-local for the duration of the run.
    // Use the same pattern as err_thread_local to avoid threading through
    // every executor function.
    crate::executor::callbacks_thread_local::install(callbacks, || {
        run_program_in_sinks(
            contents, argv0, args, label, push_main_frame,
            sink, err_sink, shell_cell,
        )
    })
}
```

Wait — that's circular. Let me restructure: add a thread-local for callbacks in `executor.rs`, similar to `err_thread_local.rs` from v206. The builtin write site consults the thread-local.

Actually the cleanest approach: add a new submodule `crates/huck-engine/src/callbacks_thread_local.rs` (mirroring `err_thread_local.rs`'s pattern from v206). The builder installs the callback pointer via an RAII guard around the run; the executor's `StdoutSink::Capture` write site checks the thread-local and dispatches.

- [ ] **Step 2 (revised): Create `crates/huck-engine/src/callbacks_thread_local.rs`**

```rust
// crates/huck-engine/src/callbacks_thread_local.rs
//! Thread-local pointer to the active `Callbacks` for the in-flight builder
//! call. Used by the executor's builtin write hook + external poll loop to
//! dispatch streaming line callbacks without threading the Callbacks reference
//! through every executor function.
//!
//! Pattern mirrors v206's `err_thread_local`. Sound because:
//!   1. Engine is `!Send + !Sync` — only one OS thread runs the executor.
//!   2. Pointer is installed for the synchronous duration of the wrapping
//!      `install` call; Guard's Drop clears even on panic.
//!   3. Callers materialize the writer in a tight scope and never store it
//!      across function boundaries.

use crate::exec_builder::Callbacks;
use std::cell::Cell;
use std::ptr::NonNull;

type CallbacksPtr = NonNull<Callbacks<'static>>;

thread_local! {
    static CALLBACKS_PTR: Cell<Option<CallbacksPtr>> = const { Cell::new(None) };
}

#[must_use = "guard must be held for the synchronous duration of the call"]
pub struct CallbacksGuard {
    prev: Option<CallbacksPtr>,
}

impl Drop for CallbacksGuard {
    fn drop(&mut self) {
        CALLBACKS_PTR.with(|c| c.set(self.prev));
    }
}

/// Install `callbacks` as the active thread-local for the returned guard's
/// lifetime. The guard's Drop restores the previous installation.
///
/// # Safety
/// The caller must hold `callbacks` alive for at least as long as the
/// returned guard. The pointer's lifetime is laundered to `'static`; the
/// guard's Drop clears it before `callbacks`'s actual lifetime ends.
pub(crate) unsafe fn install<'cb>(
    callbacks: &mut Callbacks<'cb>,
) -> CallbacksGuard {
    let raw: NonNull<Callbacks<'cb>> = NonNull::from(callbacks);
    let static_raw: NonNull<Callbacks<'static>> = unsafe { std::mem::transmute(raw) };
    let prev = CALLBACKS_PTR.with(|c| c.replace(Some(static_raw)));
    CallbacksGuard { prev }
}

/// Run `f` with the active Callbacks, if any. Returns whatever `f` returns;
/// passes `None` if no Callbacks is installed.
pub(crate) fn with_callbacks<R>(
    f: impl FnOnce(Option<&mut Callbacks<'_>>) -> R,
) -> R {
    CALLBACKS_PTR.with(|c| match c.get() {
        Some(mut p) => {
            // SAFETY: `install` guarantees `p` points to a valid Callbacks for
            // the duration of the guard, which encloses this call.
            f(Some(unsafe { p.as_mut() }))
        }
        None => f(None),
    })
}
```

- [ ] **Step 3: Register module in lib.rs**

Add `pub(crate) mod callbacks_thread_local;` to `crates/huck-engine/src/lib.rs`.

- [ ] **Step 4: Add the builtin write hook in executor.rs**

Find the `StdoutSink::Capture(buf)` write paths in `crates/huck-engine/src/executor.rs`. There are several — every place a builtin writes into the capture buffer. We add a one-line `push_stdout` call right after the buf write. To keep the change DRY, wrap the existing buf write in a helper.

Find this kind of pattern around `executor.rs:1499` and several other places:

```rust
StdoutSink::Capture(buf) => match err_sink {
    // ... existing arms write into buf ...
}
```

The cleanest approach: instead of touching every write site, hook the write at the `StdoutSink::Capture` materialization boundary. Look at the `err_writer` helper at the top of `executor.rs` (~line 70) — but that's for stderr.

Actually the cleanest hook is at `run_redirected_builtin` (or wherever the builtin gets the `&mut dyn Write` writer materialized from `StdoutSink::Capture`). The builtin writes via `out.write_all(...)` / `writeln!(out, ...)`. If we wrap that writer in a `LineDispatchWriter` that forwards to the buf AND pushes to `Callbacks::push_stdout`, we cover every builtin in one place.

Add this helper to executor.rs near the existing `err_writer` (~line 70):

```rust
/// Writer that wraps an inner `Vec<u8>` AND notifies the active
/// callbacks thread-local of any bytes written, so streaming line
/// callbacks fire as builtins write.
pub(crate) struct LineDispatchWriter<'a> {
    pub inner: &'a mut Vec<u8>,
    pub stream: LineStream,
}

#[derive(Clone, Copy)]
pub(crate) enum LineStream {
    Stdout,
    Stderr,
}

impl std::io::Write for LineDispatchWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.inner.extend_from_slice(bytes);
        let stream = self.stream;
        crate::callbacks_thread_local::with_callbacks(|cb| {
            if let Some(cb) = cb {
                match stream {
                    LineStream::Stdout => cb.push_stdout(bytes),
                    LineStream::Stderr => cb.push_stderr(bytes),
                }
            }
        });
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
```

Now find every `StdoutSink::Capture(buf) => &mut **buf` (or similar borrow into a `&mut dyn Write`) and wrap it in `LineDispatchWriter`. The primary site is `err_writer`'s Capture arm and the builtin dispatch sites. Grep:

```bash
grep -n 'StdoutSink::Capture(buf)' crates/huck-engine/src/executor.rs
```

For each occurrence where a writer is materialized for builtin output, swap:

```rust
// BEFORE
StdoutSink::Capture(buf) => &mut **buf  // or similar

// AFTER
StdoutSink::Capture(buf) => &mut LineDispatchWriter {
    inner: *buf,
    stream: LineStream::Stdout,
}
```

The exact swap depends on whether the writer is a `&mut Vec<u8>` or `Box<dyn Write>`. For `err_writer` (which returns `Box<dyn Write + 'a>`), the Box wrapping is:

```rust
StdoutSink::Capture(buf) => Box::new(LineDispatchWriter {
    inner: *buf,
    stream: LineStream::Stdout,
}),
```

Same treatment in `err_writer` for stderr's Capture arm (use `LineStream::Stderr`).

- [ ] **Step 5: Update `ExecBuilder::run_with_sinks` to install callbacks**

In `crates/huck-engine/src/exec_builder.rs`, find `run_with_sinks`. Build the Callbacks struct from `self.on_stdout_line` / `self.on_stderr_line`. Install via the thread-local around the call to `run_program_in_sinks`:

```rust
fn run_with_sinks(self, out: &mut StdoutSink, err: &mut StderrSink) -> i32 {
    let ExecBuilder {
        engine, src, stdin, merge: _, cwd, restricted, timeout,
        on_stdout_line, on_stderr_line,
    } = self;
    let cell = engine.shell_cell().clone();

    // Build callbacks. If both are None, we skip installing.
    let mut callbacks = Callbacks::new(on_stdout_line, on_stderr_line);
    let any_callbacks = callbacks.any_set();

    // Timer (v206) ...
    // [existing code]

    let code = {
        // SAFETY: callbacks is borrowed for the closure's duration;
        // CallbacksGuard's Drop runs before this scope exits.
        let _guard = if any_callbacks {
            Some(unsafe { crate::callbacks_thread_local::install(&mut callbacks) })
        } else {
            None
        };
        // [existing nested matches for stdin / cwd / restricted / run_program_in_sinks]
        // ... NO CHANGES to that body in this task — Task 5 adds the external poll loop ...
    };

    // After run: flush partial-at-EOF lines.
    callbacks.flush_partials();

    // [existing timer cancel + timeout override]
    code
}
```

- [ ] **Step 6: Add builtin-path tests in engine.rs**

Append to `crates/huck-engine/src/engine.rs::mod tests`:

```rust
#[test]
fn on_stdout_line_fires_per_line() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let out = e
        .exec("echo a; echo b; echo c")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert_eq!(out.exit_code, 0);
    assert_eq!(lines, vec!["a", "b", "c"]);
}

#[test]
fn on_stdout_line_empty_line() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.exec("echo \"\"")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert_eq!(lines, vec![""]);
}

#[test]
fn on_stdout_line_partial_at_eof() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.exec("printf 'no-newline'")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert_eq!(lines, vec!["no-newline"]);
}

#[test]
fn on_stderr_line_fires_per_line() {
    let mut out_lines: Vec<String> = Vec::new();
    let mut err_lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.exec("echo hi; echo err >&2")
        .on_stdout_line(|line| out_lines.push(line.to_string()))
        .on_stderr_line(|line| err_lines.push(line.to_string()))
        .capture();
    assert_eq!(out_lines, vec!["hi"]);
    assert_eq!(err_lines, vec!["err"]);
}

#[test]
fn on_stdout_line_captures_too() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let out = e.exec("echo a; echo b")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    // Tee: BOTH the buffer AND the callback have the lines.
    assert_eq!(out.stdout, "a\nb\n");
    assert_eq!(lines, vec!["a", "b"]);
}

#[test]
fn on_stdout_line_no_callback_capture_unchanged() {
    let mut e = Engine::new();
    let out = e.capture("echo unchanged");
    // Sanity: no-callback capture is exactly v205/v206 behavior.
    assert_eq!(out.stdout, "unchanged\n");
    assert_eq!(out.stderr, "");
    assert_eq!(out.exit_code, 0);
}
```

- [ ] **Step 7: Run tests + suite**

```bash
cargo build --workspace -q
cargo test --workspace --quiet on_stdout_line on_stderr_line
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 6 new tests pass; full suite green; clippy clean.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/{exec_builder.rs,executor.rs,engine.rs,callbacks_thread_local.rs,lib.rs}
git commit -m "$(cat <<'EOF'
v207 task 4: builtin-path streaming line dispatch hook

Callbacks { stdout, stderr, line_buf_out, line_buf_err } owned by the builder
for the call's duration. Installed via a thread-local pointer (mirrors v206's
err_thread_local pattern) so the executor's StdoutSink::Capture/StderrSink::Capture
write sites can dispatch lines without threading the callbacks reference
through every executor function.

LineDispatchWriter wraps the Vec<u8> capture buffer; its Write impl forwards
to both the inner buf AND the active callbacks (push_stdout / push_stderr)
via the thread-local. Used in err_writer's Capture arms so every builtin
write goes through line-buffering + dispatch.

ExecBuilder::run_with_sinks builds Callbacks, installs the thread-local guard
around the run, and calls flush_partials() at end to dispatch any final
no-newline line. External-process path (task 5) shares the same Callbacks.

6 new unit tests cover per-line dispatch, empty line, partial-at-EOF,
stderr, tee with capture, and v205 no-callback sanity.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: External-process poll loop

**Files:**
- Create: `crates/huck-engine/src/stream_loop.rs`
- Modify: `crates/huck-engine/src/executor.rs` (replace blocking external waits at 3 fork sites with `stream_loop::external_capture_loop`)
- Modify: `crates/huck-engine/src/lib.rs` (declare `pub(crate) mod stream_loop;`)
- Modify: `crates/huck-engine/src/engine.rs` (external-path tests)

This is the largest task. The 3 fork sites (today's `run_subprocess`, `Command::Subshell` branch, `run_multi_stage`) each spawn a child and currently block on `waitpid`. Under capture (with or without callbacks), they get a `Vec<u8>` populated via a drainer thread. We replace this with a single `external_capture_loop` helper that runs the poll loop on the embedder's thread and dispatches callbacks in real time.

- [ ] **Step 1: Create `crates/huck-engine/src/stream_loop.rs`**

```rust
// crates/huck-engine/src/stream_loop.rs
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
/// Callbacks thread-local (line-buffered). Returns the child's wait status.
///
/// `pipe_out` / `pipe_err` may be -1 if not in use (e.g. when stderr is
/// inherited or merged).
pub fn external_capture_loop(
    child_pid: libc::pid_t,
    pipe_out: RawFd,
    pipe_err: RawFd,
    sinks: CaptureSinks<'_>,
    timeout_remaining: impl FnMut() -> Option<Duration>,
) -> io::Result<i32> {
    let mut wl = WaitLoop::new()?;
    if pipe_out >= 0 { wl.register_pipe(pipe_out)?; }
    if pipe_err >= 0 { wl.register_pipe(pipe_err)?; }
    wl.register_sigchld()?;

    let mut sinks = sinks;
    let mut timeout = timeout_remaining;
    loop {
        let to = timeout();
        let events = wl.poll(to)?;
        if events.is_empty() {
            // Timeout / interrupted: just loop. Higher-level check_interrupt
            // (called between commands) will observe timeout_flag or
            // sigint_flag and abort.
            continue;
        }
        let mut child_exited = false;
        for ev in events {
            match ev {
                Event::Readable(fd) if fd == pipe_out => {
                    read_and_dispatch(fd, sinks.stdout.as_deref_mut(), true)?;
                }
                Event::Readable(fd) if fd == pipe_err => {
                    read_and_dispatch(fd, sinks.stderr.as_deref_mut(), false)?;
                }
                Event::Readable(_) => {
                    // Unknown fd — shouldn't happen.
                }
                Event::ChildExited => child_exited = true,
            }
        }
        if child_exited {
            // Reap.
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
            // SIGCHLD fired for a different child; loop.
        }
    }
}

fn read_and_dispatch(
    fd: RawFd,
    sink: Option<&mut Vec<u8>>,
    is_stdout: bool,
) -> io::Result<()> {
    let mut buf = [0u8; CHUNK_SIZE];
    loop {
        let n = unsafe {
            libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len())
        };
        if n > 0 {
            let chunk = &buf[..n as usize];
            if let Some(sink) = sink.as_deref_mut() {
                sink.extend_from_slice(chunk);
            }
            with_callbacks(|cb| {
                if let Some(cb) = cb {
                    if is_stdout { cb.push_stdout(chunk); }
                    else { cb.push_stderr(chunk); }
                }
            });
            // After a partial read (n < CHUNK_SIZE), break so we re-poll.
            // For a full read, loop to drain.
            if (n as usize) < CHUNK_SIZE { return Ok(()); }
        } else if n == 0 {
            // EOF — peer closed.
            return Ok(());
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN)
                || err.raw_os_error() == Some(libc::EWOULDBLOCK)
            {
                return Ok(()); // nothing more readable right now
            }
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
    }
}

fn drain_to_eof(
    fd: RawFd,
    sink: Option<&mut Vec<u8>>,
    is_stdout: bool,
) -> io::Result<()> {
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
                    if is_stdout { cb.push_stdout(chunk); }
                    else { cb.push_stderr(chunk); }
                }
            });
        } else if n == 0 {
            return Ok(());
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) { continue; }
            return Err(err);
        }
    }
}
```

- [ ] **Step 2: Register module in lib.rs**

Add `pub(crate) mod stream_loop;` to `crates/huck-engine/src/lib.rs`.

- [ ] **Step 3: Identify the 3 fork sites in executor.rs**

```bash
grep -n 'fn run_subprocess\|fn run_command\|fn run_multi_stage' crates/huck-engine/src/executor.rs
```

- `run_subprocess` — single external command path (around line ~5000 in v206-ended state).
- `Command::Subshell` arm in `run_command` (around line ~530).
- `run_multi_stage` — pipeline stages (around line ~5800).

- [ ] **Step 4: Replace `run_subprocess`'s blocking-wait + drainer with `external_capture_loop`**

Find the current shape: pipes opened, child spawned, `std::thread::spawn` reading the pipes, blocking `wait()`. Replace with:

```rust
// SKETCH — the actual edit must preserve the surrounding setup
// (process configuration, pre_exec, redirects, etc.). Only the
// "spawn drainer thread + block on wait" tail changes.

// After child is spawned and pipe fds are extracted:
let pipe_out = capture_stdout.as_ref().map(|p| p.read).unwrap_or(-1);
let pipe_err = capture_stderr.as_ref().map(|p| p.read).unwrap_or(-1);

let sinks = crate::stream_loop::CaptureSinks {
    stdout: stdout_buf.as_deref_mut(),
    stderr: stderr_buf.as_deref_mut(),
};

let pid = child.id() as libc::pid_t;
let status = crate::stream_loop::external_capture_loop(
    pid,
    pipe_out,
    pipe_err,
    sinks,
    || None, // timeout handled by the v206 timer thread + check_interrupt
)?;

let exit_code = if libc::WIFEXITED(status) {
    libc::WEXITSTATUS(status)
} else if libc::WIFSIGNALED(status) {
    128 + libc::WTERMSIG(status)
} else {
    1
};

// Existing PID-registry cleanup (v206) still applies.
```

- [ ] **Step 5: Replace `Command::Subshell` branch in `run_command`**

Same pattern. The subshell-fork path currently spawns a drainer thread when `StdoutSink::Capture` is active. Replace with `external_capture_loop`.

- [ ] **Step 6: Replace `run_multi_stage` (pipelines)**

The pipeline path is more complex (multiple stages, the FINAL stage's stdout is the capture). Only the final stage's pipe goes into `external_capture_loop`; the timeout cancellation still works because the LiveChildGuard registry (v206) holds every stage's PID.

- [ ] **Step 7: Add external-path tests in engine.rs**

```rust
#[test]
fn on_stdout_line_external_real_time() {
    use std::time::{Duration, Instant};
    let mut timestamps: Vec<Instant> = Vec::new();
    let mut e = Engine::new();
    let _ = e
        .exec("/bin/sh -c 'echo first; sleep 0.1; echo second'")
        .on_stdout_line(|_line| timestamps.push(Instant::now()))
        .capture();
    assert_eq!(timestamps.len(), 2);
    let gap = timestamps[1].duration_since(timestamps[0]);
    assert!(
        gap >= Duration::from_millis(50),
        "expected ~100ms gap, got {gap:?}"
    );
    assert!(
        gap <= Duration::from_secs(2),
        "gap too large: {gap:?}"
    );
}

#[test]
fn on_stdout_line_external_fires_during_wait() {
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
    let flag = Arc::new(AtomicBool::new(false));
    let mut e = Engine::new();
    let f = flag.clone();
    let _ = e
        .exec("/bin/sh -c 'echo early; sleep 0.5'")
        .on_stdout_line(move |_| f.store(true, Ordering::Relaxed))
        .capture();
    // Callback should have fired during the sleep (not after).
    assert!(flag.load(Ordering::Relaxed));
}

#[test]
fn on_stdout_line_pipeline_last_stage() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.exec("echo hi | tr a-z A-Z")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert_eq!(lines, vec!["HI"]);
}

#[test]
fn on_stdout_line_merge_stderr_routes_through_stdout() {
    let mut out_lines: Vec<String> = Vec::new();
    let mut err_lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.exec("echo a; echo b >&2")
        .merge_stderr()
        .on_stdout_line(|line| out_lines.push(line.to_string()))
        .on_stderr_line(|line| err_lines.push(line.to_string()))
        .capture();
    assert!(out_lines.contains(&"a".to_string()));
    assert!(out_lines.contains(&"b".to_string()));
    assert!(err_lines.is_empty());
}

#[test]
fn on_stdout_line_external_long_line() {
    let mut got_len: usize = 0;
    let mut e = Engine::new();
    e.exec("/bin/sh -c 'printf %0.s a {1..200000}; echo'")
        .on_stdout_line(|line| got_len = line.len())
        .capture();
    // The line should be 200,000 'a's. (Some shells may use 'a' once per
    // expansion of {1..200000}.)
    assert!(got_len >= 100_000, "expected long line, got {got_len}");
}
```

- [ ] **Step 8: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet on_stdout_line on_stderr_line
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 5 new tests pass; full suite green; clippy clean.

- [ ] **Step 9: Commit**

```bash
git add crates/huck-engine/src/{stream_loop.rs,executor.rs,engine.rs,lib.rs}
git commit -m "$(cat <<'EOF'
v207 task 5: external-process poll loop replaces blocking waitpid

New stream_loop::external_capture_loop helper: poll-based wait on stdout/stderr
pipes + SIGCHLD via WaitLoop. Reads pipe bytes as they arrive, pushes into
the optional capture buffer AND dispatches to the active Callbacks
thread-local (line-buffered). Real-time delivery; runs on the embedder's
thread; no internal drainer threads.

3 fork sites (run_subprocess, Command::Subshell, run_multi_stage) replaced
their blocking waitpid + std::thread::spawn drainer with calls to
external_capture_loop. LiveChildGuard (v206 PID registry) still handles
timeout SIGTERM and cleanup; check_interrupt still observes timeout_flag /
sigint_flag.

5 new tests cover real-time-during-wait (timestamps), fires-during-sleep
(flag), pipeline-last-stage routing, merge_stderr routing through
on_stdout_line, and very long lines (>100k chars in one callback).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `run()` tee — interpose pipe when callbacks set on .run()

**Files:**
- Modify: `crates/huck-engine/src/exec_builder.rs` (run path)
- Modify: `crates/huck-engine/src/engine.rs` (tests)

When `.on_stdout_line(cb).run()` is called, we need to interpose a pipe (since `.run()` normally inherits fd 1/2) so the engine can line-buffer and dispatch. After each callback returns, we re-write `line + "\n"` to the embedder's real fd 1/2 so the tee output reaches their terminal.

- [ ] **Step 1: In `run_with_sinks`, switch to capture-sink-with-tee when callbacks are set under run()**

The simplest approach: when callbacks are set under `.run()` (i.e. `out: StdoutSink::Terminal`), switch the sink to `Capture(&mut buf)` internally, but ALSO save real fd 1/2 and re-write bytes from the buffer back to them after each line is captured.

Concretely: add a `Callbacks::push_stdout_with_tee` variant that, after dispatching the callback, writes `line + "\n"` to the saved real fd 1. Same for stderr.

Modify `Callbacks` to optionally hold real fd writers:

```rust
pub(crate) struct Callbacks<'cb> {
    pub stdout: Option<Box<dyn FnMut(&str) + 'cb>>,
    pub stderr: Option<Box<dyn FnMut(&str) + 'cb>>,
    pub line_buf_out: crate::line_buf::LineBuf,
    pub line_buf_err: crate::line_buf::LineBuf,
    /// If `Some`, after dispatching each complete line to the stdout
    /// callback, re-write `line\n` to this fd (tee).
    pub tee_stdout_fd: Option<std::os::fd::RawFd>,
    pub tee_stderr_fd: Option<std::os::fd::RawFd>,
}
```

In `push_stdout`:

```rust
pub fn push_stdout(&mut self, bytes: &[u8]) {
    if self.stdout.is_none() && self.tee_stdout_fd.is_none() {
        return;
    }
    self.line_buf_out.push(bytes);
    while let Some(line) = self.line_buf_out.next_line() {
        if let Some(cb) = &mut self.stdout {
            cb(&line);
        }
        if let Some(fd) = self.tee_stdout_fd {
            // Re-write to the embedder's real fd 1, restoring \n.
            let bytes = line.as_bytes();
            unsafe {
                libc::write(fd, bytes.as_ptr() as *const _, bytes.len());
                libc::write(fd, b"\n".as_ptr() as *const _, 1);
            }
        }
    }
}
```

In `flush_partials`, also re-write the partial.

- [ ] **Step 2: In `ExecBuilder::run`, set up tee fds when callbacks are present**

In `exec_builder.rs`, find `run` and `capture`. The `run` arm currently builds `StdoutSink::Terminal`. When callbacks are set, instead build `StdoutSink::Capture(&mut buf)` and save fd 1 as `tee_stdout_fd`.

```rust
pub fn run(self) -> i32 {
    let any_cb = self.on_stdout_line.is_some() || self.on_stderr_line.is_some();
    if !any_cb {
        // Fast path: no callbacks, fd 1/2 inherit (v206 behavior).
        let mut out = StdoutSink::Terminal;
        let mut err = if self.merge { StderrSink::Merged } else { StderrSink::Terminal };
        return self.run_with_sinks(&mut out, &mut err);
    }
    // Slow path with tee: save real fd 1/2, route through Capture sinks.
    let saved_stdout_fd = unsafe { libc::dup(1) };
    let saved_stderr_fd = unsafe { libc::dup(2) };
    let mut tee_buf_out: Vec<u8> = Vec::new();
    let mut tee_buf_err: Vec<u8> = Vec::new();

    // Build the builder again with the tee fds set on Callbacks.
    // (Refactor: extract a `run_with_sinks_and_tee` that takes the tee fds.)
    let result = run_with_tee(
        self,
        &mut tee_buf_out,
        &mut tee_buf_err,
        saved_stdout_fd,
        saved_stderr_fd,
    );

    unsafe { libc::close(saved_stdout_fd); libc::close(saved_stderr_fd); }
    result
}
```

`run_with_tee` is `run_with_sinks` with the additional step of populating `callbacks.tee_stdout_fd = Some(saved_stdout_fd)`.

- [ ] **Step 3: Add `.run()` tee tests**

```rust
#[test]
fn on_stdout_line_run_inherits_via_tee() {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    // Redirect real fd 1 to a pipe so we can verify the tee re-write.
    let mut fds = [0; 2];
    let r = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    assert_eq!(r, 0);
    let pipe_r = fds[0];
    let pipe_w = fds[1];

    let saved = unsafe { libc::dup(1) };
    unsafe { libc::dup2(pipe_w, 1); libc::close(pipe_w); }

    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let _ = e.exec("echo tee-hi")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .run();

    unsafe { libc::dup2(saved, 1); libc::close(saved); }

    let mut buf = String::new();
    let mut file = unsafe { std::fs::File::from_raw_fd(pipe_r) };
    file.read_to_string(&mut buf).unwrap();

    assert_eq!(lines, vec!["tee-hi"]);
    assert_eq!(buf, "tee-hi\n", "embedder's real fd 1 should also see the line");
}

#[test]
fn on_stdout_line_run_no_callback_no_pipe() {
    // Sanity: no callback under run() takes the fast path.
    let mut e = Engine::new();
    let code = e.exec("true").run();
    assert_eq!(code, 0);
}
```

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet on_stdout_line_run
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 2 new tests pass; full suite green; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/{exec_builder.rs,engine.rs}
git commit -m "$(cat <<'EOF'
v207 task 6: run() tee — interpose pipe + re-write under callbacks

When .on_stdout_line() / .on_stderr_line() is set under .run() (vs .capture()),
we can no longer let fd 1/2 inherit directly because we need to line-buffer.
Instead: save real fd 1/2 via dup(), route output through Capture sinks for
line dispatch, and tee each complete line back to the saved real fds AFTER
the callback returns. Embedder still sees the script's output on their
terminal; callback fires before each line lands.

Callbacks struct gains tee_stdout_fd/tee_stderr_fd. push_stdout/push_stderr
re-write `line\n` to the tee fd after dispatching to the callback. The
no-callback fast path (run() with neither on_*_line set) is unchanged
from v206 — fd 1/2 inherit, no pipe interposition.

2 new tests cover the tee re-write (using a pipe to capture the embedder's
real fd 1) and the no-callback fast-path sanity.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Composition tests + remaining robustness

**Files:**
- Modify: `crates/huck-engine/src/engine.rs` (composition + robustness tests)

Most of the wiring is done; this task verifies the new callbacks compose with v205/v206 knobs and behaves under stress.

- [ ] **Step 1: Add the composition tests**

```rust
#[test]
fn on_stdout_line_with_stdin() {
    let _g = crate::test_support::STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let _ = e.exec("read x; echo \"got:$x\"")
        .stdin(b"hi\n".to_vec())
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert_eq!(lines, vec!["got:hi"]);
}

#[test]
fn on_stdout_line_with_cwd() {
    let _g = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let _ = e.exec("pwd")
        .cwd(tmp.path())
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    let canonical = std::fs::canonicalize(tmp.path()).unwrap();
    assert_eq!(lines, vec![canonical.display().to_string()]);
}

#[test]
fn on_stdout_line_with_restricted() {
    let mut err_lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let _ = e.exec("cd /tmp")
        .restricted(true)
        .on_stderr_line(|line| err_lines.push(line.to_string()))
        .capture();
    assert!(err_lines.iter().any(|l| l.contains("restricted: cd")));
}

#[test]
fn on_stdout_line_with_timeout_fires_during_run() {
    use std::time::Duration;
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let code = e.exec("/bin/sh -c 'echo before; sleep 5'")
        .timeout(Duration::from_millis(200))
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture()
        .exit_code;
    assert_eq!(code, 124);
    assert_eq!(lines, vec!["before"]);
}

#[test]
fn all_knobs_compose() {
    use std::time::Duration;
    let _g1 = crate::test_support::CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _g2 = crate::test_support::STDIN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let mut out_lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    let out = e.exec("read x; echo \"got:$x\"")
        .cwd(tmp.path())
        .restricted(true)
        .timeout(Duration::from_secs(2))
        .stdin(b"hello\n".to_vec())
        .on_stdout_line(|line| out_lines.push(line.to_string()))
        .capture();
    assert_eq!(out.exit_code, 0);
    assert_eq!(out_lines, vec!["got:hello"]);
}
```

- [ ] **Step 2: Add robustness tests**

```rust
#[test]
fn callback_panic_propagates_and_engine_recovers() {
    let mut e = Engine::new();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        e.exec("echo a; echo b; echo c")
            .on_stdout_line(|line| {
                if line == "b" { panic!("test panic"); }
            })
            .capture()
    }));
    assert!(r.is_err(), "expected panic to propagate out of .capture()");
    // Engine is still usable for the next call (no state corruption).
    let out = e.capture("echo recovered");
    assert_eq!(out.stdout, "recovered\n");
}

#[test]
fn callback_can_be_slow_backpressure_works() {
    use std::time::{Duration, Instant};
    let mut e = Engine::new();
    let start = Instant::now();
    let _ = e.exec("for i in $(seq 1 20); do echo $i; done")
        .on_stdout_line(|_| std::thread::sleep(Duration::from_millis(20)))
        .capture();
    let elapsed = start.elapsed();
    // 20 lines × 20ms = 400ms minimum.
    assert!(
        elapsed >= Duration::from_millis(300),
        "expected backpressure to slow run, elapsed: {elapsed:?}"
    );
}
```

- [ ] **Step 3: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 7 new tests pass; full suite green; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
v207 task 7: composition + robustness tests

7 new engine.rs tests verify on_stdout_line composes with v205/v206 knobs:
stdin (read x; echo got:hi), cwd (pwd in tmpdir), restricted (cd /tmp
diagnostic on stderr callback), timeout (lines fire before timer
aborts script), all-knobs-together. Plus robustness: callback panic
propagates and engine recovers; slow callback backpressures the script
(elapsed >= sum of callback delays).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Self-consistency harness

**Files:**
- Create: `crates/huck-engine/examples/engine_stream_diff.rs`
- Create: `tests/scripts/engine_stream_consistency_check.sh`

- [ ] **Step 1: Create the Rust driver**

```rust
// crates/huck-engine/examples/engine_stream_diff.rs
//! Self-consistency driver for v207 streaming. Runs the same fragment twice —
//! once with no callback (`Output.stdout`), once with a string-accumulating
//! callback — and emits both, so the bash harness can verify they agree.

use huck_engine::Engine;
use std::io::Write;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode arg (cap | stream)");
    let fragment = args.next().expect("fragment arg");

    let mut e = Engine::new();
    let (stdout_bytes, exit_code) = match mode.as_str() {
        "cap" => {
            let out = e.exec(&fragment).capture();
            (out.stdout.into_bytes(), out.exit_code)
        }
        "stream" => {
            let mut acc = String::new();
            let out = e
                .exec(&fragment)
                .on_stdout_line(|line| {
                    acc.push_str(line);
                    acc.push('\n');
                })
                .capture();
            // Note: the trailing-partial line (no \n) gets a \n appended here
            // for the comparison, even though Output.stdout doesn't add one.
            // To make the comparison clean, trim trailing whitespace from both
            // sides in the harness, OR only test fragments that end with \n.
            (acc.into_bytes(), out.exit_code)
        }
        _ => panic!("unknown mode: {mode}"),
    };

    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    writeln!(h, "STDOUT:{}", stdout_bytes.len()).unwrap();
    h.write_all(&stdout_bytes).unwrap();
    writeln!(h, "EXIT:{}", exit_code).unwrap();
}
```

- [ ] **Step 2: Create the bash harness**

```bash
#!/usr/bin/env bash
# Self-consistency: engine_stream_diff runs each fragment with capture (cap)
# and with streaming callback (stream). The two outputs must agree.
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --example engine_stream_diff -p huck-engine >/dev/null 2>&1
DRIVER=target/debug/examples/engine_stream_diff
if [ ! -x "$DRIVER" ]; then
    echo "FAIL: driver not found at $DRIVER" >&2
    exit 1
fi

FAIL=0
check() {
    local label=$1 frag=$2
    local cap stream
    cap=$("$DRIVER" cap "$frag")
    stream=$("$DRIVER" stream "$frag")
    if [ "$cap" != "$stream" ]; then
        echo "FAIL [$label]"
        diff <(printf '%s' "$cap") <(printf '%s' "$stream") || true
        FAIL=1
    else
        echo "PASS [$label]"
    fi
}

# All fragments end with \n so the partial-line edge doesn't trip us.
check 'builtin-only'    'echo a; echo b; echo c'
check 'external-only'   '/bin/sh -c "echo x; echo y"'
check 'mixed'           'echo bi; /bin/sh -c "echo ext"; echo bo'
check 'pipeline'        'echo hi | tr a-z A-Z'
check 'redirect-2to1'   'echo a; echo b 2>&1'
check 'long-output'     'for i in $(seq 1 50); do echo line-$i; done'

if [ $FAIL -ne 0 ]; then
    echo "engine_stream_consistency_check FAILED" >&2
    exit 1
fi
echo "engine_stream_consistency_check OK"
```

`chmod +x tests/scripts/engine_stream_consistency_check.sh`.

- [ ] **Step 3: Run the harness**

```bash
chmod +x tests/scripts/engine_stream_consistency_check.sh
bash tests/scripts/engine_stream_consistency_check.sh
```

Expected: all 6 checks PASS.

- [ ] **Step 4: Run the full suite + clippy**

```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/examples/engine_stream_diff.rs \
        tests/scripts/engine_stream_consistency_check.sh
git commit -m "$(cat <<'EOF'
v207 task 8: self-consistency harness for streaming

engine_stream_diff example binary runs each fragment with capture mode (the
v205 path) and stream mode (v207 callbacks) and emits both transcripts in the
STDOUT:<n>\n<bytes>EXIT:<code> protocol. The bash harness asserts the two
modes produce byte-identical output for 6 fragments (builtin-only,
external-only, mixed, pipeline, redirect-2to1, long-output). This is the
self-consistency property — no bash equivalent for streaming, so we verify
internally instead.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Docs + final verify

**Files:**
- Modify: `docs/architecture.md`
- Modify: `crates/huck-engine/src/engine.rs` (rustdoc example update)

- [ ] **Step 1: Append streaming paragraph to architecture.md**

In `docs/architecture.md`, find the `huck-engine` paragraph (last modified in v206) and append:

```
Streaming callbacks (v207) layer on top: `.on_stdout_line(|line| …)` and
`.on_stderr_line(…)` fire per complete line, on the embedder's thread (no
`Send` bound), in real time even for external processes. Internally, builtin
writes go through a thread-local `Callbacks` pointer that line-buffers via
`line_buf.rs`; external-process waits use a new poll-based loop
(`stream_loop.rs` + `wait_loop.rs` — `signalfd`/`poll` on Linux, `kqueue` on
macOS) that replaces v205/v206's blocking `waitpid` + drainer-thread.
Callbacks tee with `.run()` and `.capture()` — output still reaches the
embedder's terminal / `Output.stdout` buffer in addition to firing events.
```

- [ ] **Step 2: Update the rustdoc example on `Engine::exec`**

Find the existing example (added in v206). Append a streaming example:

```rust
//! // Stream output as the script runs.
//! let mut lines: Vec<String> = Vec::new();
//! let exit = e.exec("for i in 1 2 3; do echo $i; done")
//!     .on_stdout_line(|line| lines.push(line.to_string()))
//!     .run();
//! assert_eq!(exit, 0);
//! assert_eq!(lines, vec!["1", "2", "3"]);
```

- [ ] **Step 3: Run the full sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet
bash tests/scripts/engine_stream_consistency_check.sh
bash tests/scripts/engine_sandbox_diff_check.sh
bash tests/scripts/engine_capture_diff_check.sh
# All existing harnesses:
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    if [ $? -ne 0 ]; then
        echo "FAIL: $h"
        tail -20 /tmp/h.out
    fi
done
```

Expected: all green; release binary builds; no existing-harness regressions.

- [ ] **Step 4: Commit**

```bash
git add docs/architecture.md crates/huck-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
v207 task 9: architecture.md note on streaming callbacks + doc example

Architecture doc gains a paragraph on .on_stdout_line / .on_stderr_line,
line_buf.rs, wait_loop.rs, stream_loop.rs, and the tee semantics.
Engine::exec rustdoc gains a streaming example showing how to accumulate
lines from a loop. No bash-divergences.md change (embedder-facing).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Stop — do NOT merge**

Final whole-branch code review is the controller's call after Task 9. Stop after this commit; the controller will dispatch the review and ask the user to confirm before merging to main.

---

## Self-review

**Spec coverage:**
- `on_stdout_line` / `on_stderr_line` API + lifetime story: Task 3.
- `LineBuf`: Task 1.
- `WaitLoop` cross-platform: Task 2.
- Builtin-path dispatch: Task 4.
- External-process poll loop (3 fork sites): Task 5.
- Tee with `.run()`: Task 6.
- Composition with v206 knobs + robustness: Task 7.
- Self-consistency harness: Task 8.
- Doc updates: Task 9.

**Placeholder scan:** No "TBD" / "implement later" / "similar to Task N." Code blocks complete. The external-process pipeline integration (Task 5 Step 6) describes the multi-stage pipeline path approximately rather than verbatim because the surrounding code is large and exact line numbers will shift — the implementer is told what to do, what helpers to use, and what tests to pass.

**Type consistency:**
- `Callbacks<'cb>` struct shape defined in Task 4, reused in Tasks 5/6/7.
- `external_capture_loop(child_pid, pipe_out, pipe_err, CaptureSinks, timeout_fn) -> io::Result<i32>` consistent across Task 5 sites.
- `WaitLoop::{new, register_pipe, register_sigchld, poll}` consistent between Task 2 definition and Task 5 use.
- `LineBuf::{new, push, next_line, drain_final}` consistent between Task 1 and Tasks 4/5.
- `on_stdout_line: Option<Box<dyn FnMut(&str) + 'a>>` consistent between Task 3 fields and Tasks 4/5/6/7 use.

**Tasks 5 + 6 are the riskiest** — both touch executor paths that are large and partially shared with v205/v206 code. The implementer should plan to read the surrounding code carefully before editing, and may need to refactor adjacent code to fit the new helpers. If a sub-task in 5 or 6 becomes unwieldy, suggest a split into 5a/5b based on which fork site is being modified.
