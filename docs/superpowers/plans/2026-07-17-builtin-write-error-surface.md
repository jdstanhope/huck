# v308 — Builtin Write-Error Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A builtin writing to a real fd reports exactly what bash reports, in bash's wording, and never delivers failed output anywhere else.

**Architecture:** Builtin stdout bound for a real fd stops going through the process-global `io::stdout()` (which swallows EBADF, splits reporting between `write` and `flush` on a trailing newline, and retains failed bytes). It goes through a new unbuffered `FdWriter` over raw fd 1 that returns the true errno and records the first one. A single epilogue reporter emits `<name>: write error: <strerror>` + rc 1 for all builtins. v298's workarounds for the swallowing are deleted.

**Tech Stack:** Rust, `libc`, bash-diff harnesses (`tests/scripts/*_diff_check.sh`).

**Spec:** `docs/superpowers/specs/2026-07-17-builtin-write-error-surface-design.md` — read it first; it records every bash 5.2.21 and Rust behavior with the probe that established it.

**Issues:** [#186](https://github.com/jdstanhope/huck/issues/186), [#190](https://github.com/jdstanhope/huck/issues/190), [#191](https://github.com/jdstanhope/huck/issues/191).

## Global Constraints

- **Branch:** `v308-builtin-write-error-surface`. Never commit to `main`; never merge.
- **Commit trailer**, every commit, exactly: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **`cargo fmt --all`** before every commit — CI enforces `cargo fmt --all --check`.
- **⚠️ NEVER run `cargo test --workspace` or a bare `cargo test`.** This box is 1 core / 1.9 GB and it OOM-kills the session. Use `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck`.
- **Exact error wording**, one place only: `<name>: write error: <strerror>` where `<strerror>` is `crate::bash_io_error(&e)` (it strips Rust's ` (os error N)` suffix) and `<name>` is `resolved.program`. Exit status `ExecOutcome::Continue(1)`.
- **Zero-byte rule:** an empty `write` must perform **no syscall** and return `Ok(0)`. A zero-byte `write(2)` to a bad fd returns EBADF (measured), and bash is silent for `echo -n '' >&3` (rc 0).
- **Two conversion sites, both required:** `executor.rs:1493` (main `write_to_fd1` branch) and `executor.rs:1450` (the `(StdoutSink::Terminal, StderrSink::Merged)` arm). Converting only one leaves a divergent sibling path.
- **Delete, do not keep as a backstop:** `fd1_closed` (the `fcntl(1, F_GETFD)` probe), `fd1_discard` (the throwaway `Vec`), and the `stdout_flush` `Result` check.
- **Builtin stderr is NOT touched.** `err_writer` (`executor.rs:106-125`) keeps `io::stderr()` and its `Merged` → `io::stdout()` arm.
- **The read side is NOT touched.** `read` (`builtins.rs:3264`) and `mapfile` (`2968`/`2989`) look identical (`"<name>: {}", bash_io_error(&e)`) but are a *different* family — bash's read-side wording is `read error: <n>: <strerror>`, not `write error:`. Editing them here would invent a divergence. Only the 8 sites Task 2 lists change.
- **Do not add a `BufWriter`.** The spec rejects it; the writer is unbuffered.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/huck-engine/src/fd_writer.rs` | **New.** `FdWriter` + its unit tests. One job: unbuffered writes to a raw fd with faithful errno reporting and first-error recording. |
| `crates/huck-engine/src/lib.rs` | Register the module. |
| `crates/huck-engine/src/executor.rs` | Wire `FdWriter` in at both sites; single-reporter epilogue; delete v298's machinery. |
| `crates/huck-engine/src/builtins.rs` | Silence the 8 self-reporting write sites (keep their early return). |
| `tests/scripts/builtin_write_error_diff_check.sh` | Extend to bash's full measured table + the leak differential. |

---

### Task 1: `FdWriter`

**Files:**
- Create: `crates/huck-engine/src/fd_writer.rs`
- Modify: `crates/huck-engine/src/lib.rs` (add the module declaration)

**Interfaces:**
- Consumes: nothing.
- Produces — Task 2 depends on these exact signatures:
  - `pub(crate) struct FdWriter`
  - `pub(crate) fn FdWriter::new(fd: std::os::unix::io::RawFd) -> FdWriter`
  - `pub(crate) fn FdWriter::first_error(&self) -> Option<std::io::Error>`
  - `impl std::io::Write for FdWriter`

**Why it takes an `fd` parameter rather than hardcoding fd 1:** so the unit tests can point it at a temporary fd. A test that swaps the process-global fd 1 cannot be reliable in a `#[cfg(test)]` module — `dup2` clears `O_CLOEXEC`, so concurrently forking tests inherit it (this is a known, previously-hit hazard in this repo; see `tests/tee_inherit.rs`). Keeping the fd a parameter avoids that entirely.

- [ ] **Step 1: Write the failing tests**

Create `crates/huck-engine/src/fd_writer.rs` with the tests below and no implementation yet.

```rust
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
        let e = w.write_all(b"x").expect_err("write to a read-only fd must fail");
        assert_eq!(e.raw_os_error(), Some(libc::EBADF));
        unsafe { libc::close(fd) };
    }

    #[test]
    fn write_to_closed_fd_surfaces_ebadf() {
        let fd = ro_fd();
        unsafe { libc::close(fd) };
        let mut w = FdWriter::new(fd);
        let e = w.write_all(b"x").expect_err("write to a closed fd must fail");
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-engine --jobs 1 --lib fd_writer -- --test-threads 1`
Expected: FAIL to compile — `cannot find struct FdWriter in this scope`.

- [ ] **Step 3: Write the implementation**

Insert this into `crates/huck-engine/src/fd_writer.rs`, above the `#[cfg(test)] mod tests` block (keep the module doc-comment at the top of the file):

```rust
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
            let n = unsafe {
                libc::write(self.fd, buf.as_ptr() as *const libc::c_void, buf.len())
            };
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
```

- [ ] **Step 4: Register the module**

In `crates/huck-engine/src/lib.rs`, add this line so it stays alphabetical (it goes between `pub mod exec_builder;` on line 27 and `pub(crate) mod executor;` on line 28):

```rust
pub(crate) mod fd_writer;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p huck-engine --jobs 1 --lib fd_writer -- --test-threads 1`
Expected: PASS, 8 passed.

- [ ] **Step 6: Verify the zero-byte test is not vacuous**

A test must fail when the code is wrong. Temporarily delete the `if buf.is_empty() { return Ok(0); }` short-circuit and re-run:

Run: `cargo test -p huck-engine --jobs 1 --lib fd_writer -- --test-threads 1`
Expected: `empty_write_performs_no_syscall_on_a_bad_fd` **FAILS**. Restore the short-circuit and confirm PASS again before committing.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/fd_writer.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
feat: FdWriter — unbuffered raw-fd writer with faithful errno (#186 #190 #191)

io::stdout() cannot serve builtin stdout: it swallows EBADF, splits error
reporting between write and flush on a trailing newline, and retains failed
bytes. FdWriter writes directly, returns the true errno, records the first one,
and buffers nothing.

Empty writes short-circuit without a syscall: a zero-byte write(2) to a bad fd
returns EBADF, but bash is silent for `echo -n '' >&3`.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Wire the writer in; single reporter; delete v298's machinery

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (lines ~1370-1383, ~1446-1453, ~1485-1495, ~1549-1588, and the comment at ~1327-1331)
- Modify: `crates/huck-engine/src/builtins.rs` (8 sites, listed below)
- Test: `tests/builtin_write_error_integration.rs` (**new**)

**Interfaces:**
- Consumes: `FdWriter::new(fd) -> FdWriter`, `FdWriter::first_error(&self) -> Option<io::Error>`, `impl io::Write for FdWriter` (Task 1).
- Produces: nothing later tasks depend on structurally; Task 3 gates the same behavior byte-for-byte against bash.

**Why the wiring and the silencing are one task:** once `FdWriter` is in place every failure surfaces at the builtin's own `write_all`. If the builtins still reported, every case would print the message **twice**. The two halves cannot be independently correct, so they land together.

- [ ] **Step 1: Write the failing test**

Create `tests/builtin_write_error_integration.rs`. This is a `-p huck` integration binary (it drives the real `huck` binary), so it must be run explicitly — `--lib` runs skip it.

```rust
//! v308 (#186 #190 #191): a builtin writing to a real fd that cannot be written
//! reports bash's `<name>: write error: <strerror>` + rc 1, and its failed
//! output reaches NOTHING — least of all the fd 1 that the redirect scope
//! restores afterwards.
//!
//! `exec 3</etc/hostname` gives a genuinely read-only fd (the kernel returns
//! EBADF for a write); `/dev/full` gives ENOSPC. Externals are already correct,
//! so these all exercise the in-process builtin path.

use std::process::Command;

fn huck() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run `huck -c script`, returning (stdout, stderr, rc).
fn run(script: &str) -> (String, String, i32) {
    let out = Command::new(huck())
        .arg("-c")
        .arg(script)
        .output()
        .expect("spawn huck");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn echo_to_read_only_fd_reports_and_fails() {
    let (out, err, rc) = run("exec 3</etc/hostname; echo x >&3");
    assert!(
        err.contains("echo: write error: Bad file descriptor"),
        "stderr was: {err:?}"
    );
    assert_eq!(rc, 1, "exit status");
    assert_eq!(out, "", "nothing may reach the real stdout");
}

#[test]
fn printf_to_read_only_fd_reports_and_fails() {
    let (_, err, rc) = run("exec 3</etc/hostname; printf x >&3");
    assert!(
        err.contains("printf: write error: Bad file descriptor"),
        "stderr was: {err:?}"
    );
    assert_eq!(rc, 1);
}

/// `declare` discards its own write Result (one of ~82 such sites), so this
/// only works if the WRITER records the error rather than the builtin checking.
#[test]
fn declare_to_read_only_fd_reports_and_fails() {
    let (_, err, rc) = run("exec 3</etc/hostname; x=1; declare -p x >&3");
    assert!(
        err.contains("declare: write error: Bad file descriptor"),
        "stderr was: {err:?}"
    );
    assert_eq!(rc, 1);
}

/// #191: the payload must never reach the restored fd 1. All four shapes leaked
/// before this change.
#[test]
fn failed_output_never_leaks_to_the_restored_fd1() {
    for script in [
        "echo x > /dev/full",
        "echo -n x > /dev/full",
        "printf 'x' > /dev/full",
        "x=1; declare -p x > /dev/full",
    ] {
        let (out, _, rc) = run(script);
        assert_eq!(out, "", "payload leaked to the real stdout for: {script}");
        assert_eq!(rc, 1, "exit status for: {script}");
    }
}

/// #190: the wording must not depend on a trailing newline. Before this change
/// the newline chose the reporter, and the two disagreed.
#[test]
fn wording_is_identical_with_and_without_a_trailing_newline() {
    for script in [
        "printf 'x\\n' > /dev/full",
        "printf 'x' > /dev/full",
        "echo x > /dev/full",
        "echo -n x > /dev/full",
    ] {
        let (_, err, rc) = run(script);
        assert!(
            err.contains("write error: No space left on device"),
            "wrong wording for {script:?}: {err:?}"
        );
        assert!(
            !err.contains("os error"),
            "raw io::Error leaked into the message for {script:?}: {err:?}"
        );
        assert_eq!(rc, 1);
    }
}

/// bash is SILENT when no bytes are written, even to a broken fd.
#[test]
fn zero_byte_output_is_silent_and_succeeds() {
    for script in [
        "exec 3</etc/hostname; echo -n '' >&3",
        "exec 3</etc/hostname; printf '' >&3",
        "exec 3</etc/hostname; : >&3",
        "exec 3</etc/hostname; true >&3",
        "exec 3</etc/hostname; jobs >&3",
        "exec 3</etc/hostname; cd /tmp >&3",
    ] {
        let (_, err, rc) = run(script);
        assert!(!err.contains("write error"), "must be silent: {script}");
        assert_eq!(rc, 0, "exit status for: {script}");
    }
}

/// A writable fd must not be reported (guards against false positives).
#[test]
fn writable_fd_is_not_reported() {
    let (_, err, rc) = run("exec 3<>/tmp/huck-v308-rw.txt; echo x >&3");
    assert!(!err.contains("write error"), "stderr was: {err:?}");
    assert_eq!(rc, 0);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `ulimit -v 6000000; cargo build -p huck && cargo test -p huck --test builtin_write_error_integration --jobs 1 -- --test-threads 1`
Expected: FAIL. `echo_to_read_only_fd_reports_and_fails` fails on the missing message (huck is silent, rc 0 — this is #186); `failed_output_never_leaks_to_the_restored_fd1` fails with `out` = `"x\n"` (this is #191); `wording_is_identical_…` fails on `os error` (this is #190).

- [ ] **Step 3: Hoist the writer and delete the v298 probe**

In `crates/huck-engine/src/executor.rs`, replace lines 1370-1383 — the `#137` comment block, the `fd1_closed` binding, and the `fd1_discard` binding — with:

```rust
    // #186/#190/#191: builtin stdout bound for a real fd goes through
    // `FdWriter`, NOT the process-global `io::stdout()`. `io::stdout()` swallows
    // EBADF (`std::io::stdio::handle_ebadf` upstream reports success for a write
    // that genuinely failed), it is a `LineWriter` — so a trailing newline
    // decides whether an error surfaces at `write_all` or at a later `flush` —
    // and it RETAINS failed bytes, which then reach whatever fd 1 is restored
    // to. `FdWriter` returns the true errno, buffers nothing, and records the
    // first error for the epilogue below. This replaces v298's (#137) `fcntl`
    // closed-fd probe and throwaway-buffer workaround: a raw write(2) reports
    // EBADF for a closed fd on its own.
    let mut fd1_writer = crate::fd_writer::FdWriter::new(libc::STDOUT_FILENO);
```

- [ ] **Step 4: Convert both sites**

First site — in `executor.rs`, the `(StdoutSink::Terminal, StderrSink::Merged)` arm (was ~1446-1453). Replace:

```rust
            (StdoutSink::Terminal, StderrSink::Merged) => {
                // Merged + terminal stdout: writes go to real fd 1 (which the
                // redirect dup'd from real fd 2, so → real fd 2). This matches
                // the non-routed path, so just fall back to the standard write.
                let mut out = io::stdout();
                let mut err = err_writer(err_sink, sink);
                run(&mut out, &mut *err, shell)
            }
```

with:

```rust
            (StdoutSink::Terminal, StderrSink::Merged) => {
                // Merged + terminal stdout: writes go to real fd 1 (which the
                // redirect dup'd from real fd 2, so → real fd 2). This matches
                // the non-routed path, so use the same `FdWriter` — it is a real
                // fd, and a sibling of the `write_to_fd1` branch below.
                let mut err = err_writer(err_sink, sink);
                run(&mut fd1_writer, &mut *err, shell)
            }
```

Second site — the `write_to_fd1` branch (was ~1485-1495). Replace:

```rust
    } else if write_to_fd1 {
        let mut err = err_writer(err_sink, sink);
        if fd1_closed {
            // Real fd 1 is already closed — see the `fd1_closed` comment above.
            // Route through a throwaway buffer rather than the EBADF-swallowing
            // `io::stdout()` so the epilogue can detect an attempted write.
            run(&mut fd1_discard, &mut *err, shell)
        } else {
            let mut out = io::stdout();
            run(&mut out, &mut *err, shell)
        }
    } else {
```

with:

```rust
    } else if write_to_fd1 {
        let mut err = err_writer(err_sink, sink);
        run(&mut fd1_writer, &mut *err, shell)
    } else {
```

- [ ] **Step 5: Replace the epilogue with the single reporter**

In `executor.rs`, replace the whole `#137` epilogue block (was ~1549-1588: the comment, `let stdout_flush = …`, the stderr flush, `fd1_write_failed`, and the `let outcome = if write_to_fd1 && …` expression) with:

```rust
    // Keep flushing `io::stdout()` here even though builtin stdout no longer
    // goes through it: `err_writer`'s `StderrSink::Merged` arm still writes
    // DIAGNOSTICS through it, and those must land before `drop(scope)` restores
    // fd 1 — otherwise they would be flushed to the restored fd, i.e. the wrong
    // destination (#191's failure mode, on the stderr side).
    let _ = io::stdout().flush();
    let _ = std::io::Write::flush(&mut std::io::stderr());
    // The SINGLE reporter for every builtin write failure. `FdWriter` recorded
    // the first errno, which matters because ~82 builtin write sites discard
    // their own `Result` (`let _ = writeln!(out, …)`) — only 6 check it — so a
    // per-builtin check could never cover `declare -p x >&3` and friends. The 6
    // that check keep their early return but no longer emit; this is the only
    // place a write error is worded, which is what keeps #190 fixed.
    let outcome = match fd1_writer.first_error() {
        Some(e) => {
            {
                let mut ew = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *ew,
                    None,
                    "{}: write error: {}",
                    resolved.program,
                    crate::bash_io_error(&e)
                );
            }
            ExecOutcome::Continue(1)
        }
        None => outcome,
    };
```

- [ ] **Step 6: Update the flush comment at line ~1327-1331 to state BOTH reasons**

The flush that opens `run_builtin_with_redirects` now carries a second, load-bearing obligation. An unstated rationale is one that rots. Replace:

```rust
    // Flush buffered terminal/builtin output BEFORE swapping fds so prior
    // output is not diverted into the redirect target.
    let _ = io::stdout().flush();
```

with:

```rust
    // Flush buffered terminal/builtin output BEFORE swapping fds, for two
    // reasons. (1) Prior output must not be diverted into the redirect target.
    // (2) v308: the builtin's own stdout now goes straight to real fd 1 via
    // `FdWriter` (unbuffered), so anything still sitting in `io::stdout()`'s
    // buffer would be overtaken by those raw writes and surface out of order.
    // Emptying the buffer here is what keeps the two writers in step.
    let _ = io::stdout().flush();
```

- [ ] **Step 7: Silence the 8 self-reporting sites in `builtins.rs`**

Each keeps its early return (stop writing once the fd is broken) and drops only its message — the epilogue now words it. Bind the error to `_e` where the value becomes unused, and delete the `sh_error_to!` line.

`builtins.rs:654-657` (`pwd`):
```rust
    if writeln!(out, "{path}").is_err() {
        // v308: the write error is reported once, by the run_builtin_with_redirects
        // epilogue (it holds the recorded errno). Stop writing; stay silent.
        return ExecOutcome::Continue(1);
    }
```

`builtins.rs:679-686` (`echo`, both sites):
```rust
    if out.write_all(&bytes).is_err() {
        // v308: reported once by the epilogue (see pwd above).
        return ExecOutcome::Continue(1);
    }
    if !suppress_newline && out.write_all(b"\n").is_err() {
        return ExecOutcome::Continue(1);
    }
```

`builtins.rs:1158-1161` (`export`):
```rust
        if writeln!(out, "{}", format_declare_line(name, var)).is_err() {
            // v308: reported once by the epilogue.
            return ExecOutcome::Continue(1);
        }
```

`builtins.rs:1824-1827` (`readonly`):
```rust
            if writeln!(out, "{line}").is_err() {
                // v308: reported once by the epilogue.
                return ExecOutcome::Continue(1);
            }
```

`builtins.rs:4283-4286` (`jobs`):
```rust
        if write_result.is_err() {
            // v308: reported once by the epilogue.
            return ExecOutcome::Continue(1);
        }
```

`builtins.rs:4143-4146` (`printf`'s write — the `(os error 28)` source):
```rust
    } else if out.write_all(&buf).is_err() {
        // v308: reported once by the epilogue, with bash's wording. This site
        // used the raw io::Error Display, which appended Rust's "(os error N)".
        return ExecOutcome::Continue(1);
    }
```

**Leave `builtins.rs:4025` (`crate::sh_error_to!(shell, err, None, "printf: {e}");`) ALONE.** Read its surrounding lines first: it reports a *format-parse* failure (`Err(e)` from parsing the format string), not a write failure. It is not part of this surface.

- [ ] **Step 8: Run the new test to verify it passes**

Run: `ulimit -v 6000000; cargo build -p huck && cargo test -p huck --test builtin_write_error_integration --jobs 1 -- --test-threads 1`
Expected: PASS, 7 passed.

- [ ] **Step 9: Verify v298's cases still pass with its machinery deleted**

This is the proof the deletion lost nothing.

Run: `bash tests/scripts/builtin_write_error_diff_check.sh`
Expected: `builtin_write_error_diff_check OK` (5/5 PASS).

- [ ] **Step 10: Run the engine lib tests + the fd/redirect integration binaries**

Run each separately — never `--workspace`:
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
ulimit -v 6000000
for t in heredoc_integration compound_redirects_integration fd_dup_integration \
         named_fd_integration builtin_fd_ordering_integration io_error_integration \
         noclobber_integration captured_pipeline_drain_integration; do
  cargo test -p huck --test $t --jobs 1 -- --test-threads 1 || echo "FAILED: $t"
done
```
Expected: all green, no `FAILED:` lines.

- [ ] **Step 11: Verify no `io::stdout()` remains in the builtin stdout path**

Run: `grep -n 'io::stdout()' crates/huck-engine/src/executor.rs`
Expected: the remaining hits are only the flush at ~1331, the flush in the epilogue, `err_writer`'s `Merged` arm (~117, builtin **stderr** — deliberately unchanged), and flushes in other functions (~158, ~1103, ~1241, ~1274). **No `let mut out = io::stdout();` may remain.**

Also confirm the deletions are complete:

Run: `grep -rn 'fd1_closed\|fd1_discard\|stdout_flush' crates/huck-engine/src/`
Expected: **no output.**

- [ ] **Step 12: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs crates/huck-engine/src/builtins.rs tests/builtin_write_error_integration.rs
git commit -m "$(cat <<'EOF'
fix: route builtin stdout through FdWriter; one reporter (#186 #190 #191)

Both sites that hand builtin stdout to a real fd — the write_to_fd1 branch and
the (Terminal, Merged) arm — now use FdWriter. Consequences:

- #186: a raw write(2) returns EBADF instead of io::stdout()'s lie, so a
  read-only fd reports and returns 1. v298's fcntl closed-fd probe and
  fd1_discard buffer are deleted, not extended: a raw write reports EBADF for a
  closed fd on its own.
- #191: nothing is buffered, so no failed payload survives to be flushed to the
  restored fd 1.
- #190: with no LineWriter, a trailing newline no longer selects the reporter.
  The 8 builtin sites that self-reported now stay silent (keeping their early
  return) and the epilogue words every failure once, from the recorded errno —
  which is also what finally covers the ~82 sites that discard their write
  Result.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: bash-diff harness — the full measured table

**Files:**
- Modify: `tests/scripts/builtin_write_error_diff_check.sh`

**Interfaces:**
- Consumes: the behavior Task 2 produced.
- Produces: the byte-identical gate. `tests/scripts/run_diff_checks.sh` picks the file up automatically via its `*_diff_check.sh` glob — no registration needed.

**Why a harness on top of Task 2's Rust test:** the Rust test asserts huck's behavior against strings *this plan* wrote down. The harness asserts it against **bash itself**, byte for byte, which is this project's gold standard.

- [ ] **Step 1: Update the header comment (it describes the deleted mechanism)**

Replace lines 2-6 of `tests/scripts/builtin_write_error_diff_check.sh`:

```bash
# v298 (#137) + v308 (#186 #190 #191): a builtin whose stdout write fails must
# report `<name>: write error: <strerror>` and exit 1, matching bash — and must
# deliver NOTHING to the real stdout. Builtin stdout goes through an unbuffered
# FdWriter over raw fd 1 (v308), so the errno is faithful (io::stdout() swallows
# EBADF) and no failed bytes remain to leak; run_builtin_with_redirects'
# epilogue is the single reporter. Compares stdout+stderr+rc byte-identically.
```

- [ ] **Step 2: Add the read-only-fd cases (#186)**

Append after the existing `check 'echo-ok' 'echo hi'` line. `RO='exec 3</etc/hostname;'` gives a genuinely read-only fd.

```bash
# --- v308 #186: an OPEN but read-only fd. bash: `<name>: write error: Bad file
# descriptor` + rc 1. huck was silent with rc 0 before v308.
RO='exec 3</etc/hostname;'
check 'ro-echo'       "$RO"' echo x >&3'
check 'ro-echo-n'     "$RO"' echo -n x >&3'
check 'ro-printf'     "$RO"' printf x >&3'
check 'ro-printf-nl'  "$RO"' printf "x\n" >&3'
check 'ro-pwd'        "$RO"' pwd >&3'
# `declare`/`export` DISCARD their own write Result (~82 such sites), so these
# pass only because the WRITER records the errno.
check 'ro-declare'    "$RO"' x=1; declare -p x >&3'
check 'ro-export'     "$RO"' export -p >&3'
# Reported once PER INVOCATION — bash prints the message twice here.
check 'ro-echo-twice' "$RO"' echo x >&3; echo x >&3'

# --- Zero bytes written: bash attempts no write(2), so it is SILENT (rc 0).
# These guard the FdWriter empty-write short-circuit; without it a zero-byte
# write(2) to a bad fd returns EBADF and huck would report where bash does not.
check 'ro-echo-empty'   "$RO"' echo -n "" >&3'
check 'ro-printf-empty' "$RO"' printf "" >&3'
check 'ro-colon'        "$RO"' : >&3'
check 'ro-true'         "$RO"' true >&3'
check 'ro-jobs-none'    "$RO"' jobs >&3'
check 'ro-cd'           "$RO"' cd /tmp >&3'
# A builtin that fails for an UNRELATED reason writes nothing to fd 1, so bash
# reports only its own error — no write error.
check 'ro-declare-nope' "$RO"' declare -p NOPE >&3'
# Control: an O_RDWR fd is writable — no error, no false positive.
check 'rw-ok'           'exec 3<>/tmp/huck-v308-rw.txt; echo x >&3'

# --- v308 #190: ENOSPC via /dev/full. The wording must NOT depend on a trailing
# newline (before v308 the newline chose the reporter and the two disagreed —
# one omitted `write error: `, the other leaked Rust's `(os error 28)`).
check 'full-echo'       'echo x > /dev/full'
check 'full-echo-n'     'echo -n x > /dev/full'
check 'full-printf'     'printf "x" > /dev/full'
check 'full-printf-nl'  'printf "x\n" > /dev/full'
check 'full-declare'    'x=1; declare -p x > /dev/full'
```

- [ ] **Step 3: Add the leak differential (#191)**

`check` merges stderr into stdout, so it cannot see a leak. Add a stdout-only comparator. Insert it after the `check()` function definition:

```bash
# #191: a FAILED write must deliver nothing to the real stdout. `check` folds
# stderr into stdout, so it cannot see a leak; compare stdout ALONE. Before
# v308, io::stdout()'s LineWriter retained the failed bytes and flushed them to
# fd 1 once the redirect scope restored it.
check_stdout() {
  local label=$1 frag=$2 b h
  b=$(timeout 10 bash -c "$frag" 2>/dev/null | od -c)
  h=$(timeout 10 "$HUCK" -c "$frag" 2>/dev/null | od -c)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label] (stdout leak)"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}
```

And append these cases at the end, before the final `if [ $FAIL -ne 0 ]` block. All four leaked before v308:

```bash
# --- v308 #191: no payload on the real stdout when the write failed.
check_stdout 'leak-echo'      'echo x > /dev/full'
check_stdout 'leak-echo-n'    'echo -n x > /dev/full'
check_stdout 'leak-printf'    'printf "x" > /dev/full'
check_stdout 'leak-declare'   'x=1; declare -p x > /dev/full'
check_stdout 'leak-ro-echo'   'exec 3</etc/hostname; echo x >&3'
```

- [ ] **Step 4: Run the harness**

Run: `cargo build -p huck && bash tests/scripts/builtin_write_error_diff_check.sh`
Expected: `builtin_write_error_diff_check OK`, 31 PASS lines, 0 FAIL.

If a `full-*` case fails because `/dev/full` is unavailable, stop and report — do not delete the case.

- [ ] **Step 5: Prove the harness would catch a regression**

A harness that cannot fail is not a gate. Temporarily revert one line — in `crates/huck-engine/src/executor.rs`, change the `write_to_fd1` branch back to `let mut out = io::stdout(); run(&mut out, &mut *err, shell)` — rebuild, and re-run:

Run: `cargo build -p huck && bash tests/scripts/builtin_write_error_diff_check.sh`
Expected: **FAIL** on the `ro-*` and `leak-*` cases. Restore the line, rebuild, and confirm OK again before committing.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add tests/scripts/builtin_write_error_diff_check.sh
git commit -m "$(cat <<'EOF'
test: pin the builtin write-error surface against bash (#186 #190 #191)

Extends v298's harness to bash's full measured table: read-only fd (echo,
printf, pwd, declare -p, export -p, double-report), the six zero-byte cases
bash stays silent for, an O_RDWR control, and ENOSPC via /dev/full with
newline and no-newline variants of both echo and printf — that pair is what
distinguished #190's two disagreeing reporters.

Adds check_stdout for #191: `check` folds stderr into stdout and so cannot see
a leaked payload. Compares stdout alone, byte for byte, for the five shapes
that leaked.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Verification (controller, before the PR)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo build -p huck --locked` and `cargo build --release -p huck --locked` — both needed: 8 harnesses default to the release binary.
- [ ] `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
- [ ] Every `-p huck` integration binary, each single-threaded with a `ulimit -v` guard.
- [ ] `tools/redirect_audit.sh`, `tools/pipeline_redirect_audit.sh`, `tools/bg_pipeline_redirect_audit.sh` — the differential gate for fd changes; expect 0 DIVERGE.
- [ ] `tests/scripts/run_diff_checks.sh` on both binaries — expect green (the lone known flake is `pipeline_stage_redirect_fail_diff_check.sh` case `amb-stdin-mid`, [#180](https://github.com/jdstanhope/huck/issues/180)).
- [ ] `grep -rn 'fd1_closed\|fd1_discard\|stdout_flush' crates/` — no output.
- [ ] PR with `Closes #186`, `Closes #190`, `Closes #191`; **the user merges, not you.** Wait for CI to finish and pass before saying it is ready.
