# `exec` heredoc hang (#169): pipe-or-tempfile body delivery — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace huck's fork-per-heredoc writer with bash's model — a pipe written directly by the parent for bodies ≤ 65536 bytes, an unlinked temp file above that — making the [#169](https://github.com/jdstanhope/huck/issues/169) `exec` hang unreachable by construction.

**Architecture:** `spawn_heredoc_writer(bytes) -> (RawFd, pid_t)` becomes `heredoc_body_to_fd(bytes, tmpdir) -> RawFd`. No fork, no pid, so nothing exists for `exec` to `waitpid` on. The body is fully delivered before the helper returns. The `heredoc_writers: Vec<pid_t>` plumbing threaded through `RedirectScope`, `RedirPlan`, `ChildRedirPlan`, `SpawnedPipeline`, and the pipeline spawn paths then has no producer and is deleted.

**Tech Stack:** Rust, `libc` (raw `pipe`/`fcntl`/`mkstemp`/`open`/`unlink`), `crate::child_fd::make_pipe`. Bash-diff harnesses are bash scripts under `tests/scripts/`.

**Spec:** `docs/superpowers/specs/2026-07-16-exec-heredoc-tempfile-design.md` (commit `f2a7a48`). Read it before Task 1 — it records the bash 5.2.21 behavior each decision is pinned to, all of it verified empirically rather than assumed.

## Global Constraints

- **Issue:** [#169](https://github.com/jdstanhope/huck/issues/169). The PR body must say `Closes #169`.
- **Branch:** `v307-exec-heredoc-tempfile`. Never push to `main`; never merge the PR yourself — the user merges.
- **Commit trailer**, on every commit, verbatim:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Formatting:** run `cargo fmt --all` before every commit. CI enforces `cargo fmt --all --check`.
- **Threshold constant:** `HEREDOC_PIPESIZE = 65536`, compared against `bytes.len()` with `<=`. This is bash's `herelen`, which *includes* the here-string's appended newline — the call sites already `bytes.push(b'\n')` before calling.
- **NEVER run `cargo test --workspace`** — this box (1 core / 1.9 GB) OOM-kills the session. Always per-crate, single-threaded:
  `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`
- **Every hang test wraps huck in `timeout`** so a regression FAILS rather than wedging CI.
- **`TMPDIR` is read from the SHELL variable** (`shell.lookup_var("TMPDIR")`), never `std::env::var`. Verified: bash honors an in-shell `TMPDIR=/x` for heredoc temp files whether exported or not, and huck does not sync exports to the process env.
- The returned fd is **read-only** on both paths.

---

### Task 1: The `heredoc_body_to_fd` helper (not yet wired)

Adds the helper and its unit tests. Nothing calls it yet, so behavior is unchanged and this task is independently reviewable as "is the mechanism correct".

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — add the helper next to the existing `spawn_heredoc_writer` (around `:3510`, under the `----- redirect file handling -----` banner); register the test module beside the others at `:8460`.
- Create: `crates/huck-engine/src/executor/heredoc_body_tests.rs`

**Interfaces:**
- Consumes: `crate::child_fd::make_pipe(cloexec: bool) -> io::Result<(RawFd, RawFd)>` (existing, `pub(crate)`).
- Produces, for Task 2:
  - `fn heredoc_body_to_fd(bytes: &[u8], tmpdir: Option<&str>) -> Result<RawFd, io::Error>`
  - `const HEREDOC_PIPESIZE: usize = 65536;`

- [ ] **Step 1: Write the failing tests**

Create `crates/huck-engine/src/executor/heredoc_body_tests.rs`:

```rust
//! #169: `heredoc_body_to_fd` delivers a heredoc/here-string body WITHOUT a
//! forked writer — a pipe for bodies <= HEREDOC_PIPESIZE, an unlinked temp file
//! above it. These tests pin the path SELECTION (via `fstat`), which a bash-diff
//! harness structurally cannot check: the temp file is unlinked and its path
//! differs per process, so it can never be byte-identical to bash's.

use super::{heredoc_body_to_fd, HEREDOC_PIPESIZE};
use std::os::fd::RawFd;

/// The st_mode file-type bits of `fd` (S_IFIFO for a pipe, S_IFREG for a file).
fn fd_kind(fd: RawFd) -> libc::mode_t {
    // SAFETY: `st` is zeroed POD and `fd` is open; fstat only writes `st`.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::fstat(fd, &mut st) };
    assert_eq!(r, 0, "fstat failed: {}", std::io::Error::last_os_error());
    st.st_mode & libc::S_IFMT
}

/// Drain `fd` to EOF and close it.
fn read_all(fd: RawFd) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        // SAFETY: `buf` is a live local; `fd` is open.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        assert!(n >= 0, "read failed: {}", std::io::Error::last_os_error());
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    // SAFETY: `fd` is open and owned by this test.
    unsafe { libc::close(fd) };
    out
}

fn body(n: usize) -> Vec<u8> {
    vec![b'x'; n]
}

#[test]
fn body_at_threshold_uses_a_pipe() {
    // bash: herelen <= 65536 -> pipe. Verified on 5.2.21 via readlink /proc/$$/fd/3.
    let b = body(HEREDOC_PIPESIZE);
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    assert_eq!(fd_kind(fd), libc::S_IFIFO, "body of exactly 65536 must be a pipe");
    assert_eq!(read_all(fd), b);
}

#[test]
fn body_over_threshold_uses_a_regular_file() {
    // bash: herelen > 65536 -> unlinked temp file. This is the #169 case: with a
    // file there is no writer to block on, so `exec 3<<<BIG` cannot hang.
    let b = body(HEREDOC_PIPESIZE + 1);
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    assert_eq!(fd_kind(fd), libc::S_IFREG, "body of 65537 must be a temp file");
    assert_eq!(read_all(fd), b);
}

#[test]
fn small_body_round_trips_through_the_pipe() {
    let b = b"hello\nworld\n".to_vec();
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    assert_eq!(fd_kind(fd), libc::S_IFIFO);
    assert_eq!(read_all(fd), b);
}

#[test]
fn empty_body_yields_an_immediately_empty_pipe() {
    let fd = heredoc_body_to_fd(&[], None).expect("heredoc_body_to_fd");
    assert_eq!(fd_kind(fd), libc::S_IFIFO);
    assert!(read_all(fd).is_empty());
}

#[test]
fn pipe_path_fd_is_read_only() {
    // bash: `exec 3<<<hi; echo x >&3` -> "write error: Bad file descriptor".
    let fd = heredoc_body_to_fd(b"hi\n", None).expect("heredoc_body_to_fd");
    // SAFETY: `fd` is open; writing 1 byte from a live local.
    let n = unsafe { libc::write(fd, b"x".as_ptr() as *const libc::c_void, 1) };
    let err = std::io::Error::last_os_error();
    assert_eq!(n, -1, "a heredoc pipe fd must not be writable");
    assert_eq!(err.raw_os_error(), Some(libc::EBADF), "err: {err}");
    unsafe { libc::close(fd) };
}

#[test]
fn tempfile_path_fd_is_read_only() {
    // bash's temp-file fd has access mode O_RDONLY (fdinfo `flags: 0100000`), so
    // writing to it fails EBADF exactly as in the pipe case.
    let b = body(HEREDOC_PIPESIZE + 1);
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    // SAFETY: `fd` is open; writing 1 byte from a live local.
    let n = unsafe { libc::write(fd, b"x".as_ptr() as *const libc::c_void, 1) };
    let err = std::io::Error::last_os_error();
    assert_eq!(n, -1, "a heredoc temp-file fd must not be writable");
    assert_eq!(err.raw_os_error(), Some(libc::EBADF), "err: {err}");
    unsafe { libc::close(fd) };
}

#[test]
fn tempfile_starts_at_offset_zero() {
    // bash reopens the file O_RDONLY (rather than rewinding the writable fd), so
    // the reader starts at 0. Guard: the FIRST bytes read are the body's first.
    let mut b = body(HEREDOC_PIPESIZE + 1);
    b[0] = b'A';
    let fd = heredoc_body_to_fd(&b, None).expect("heredoc_body_to_fd");
    let got = read_all(fd);
    assert_eq!(got.len(), b.len());
    assert_eq!(got[0], b'A', "temp-file fd must start at offset 0");
}

#[cfg(target_os = "linux")]
#[test]
fn tempfile_honors_tmpdir_and_is_unlinked() {
    // bash: TMPDIR is honored (shell variable, exported or not) and the file is
    // unlinked immediately -> readlink shows "<path> (deleted)".
    let dir = std::env::temp_dir().join(format!("huck-t1-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let b = body(HEREDOC_PIPESIZE + 1);
    let fd = heredoc_body_to_fd(&b, Some(dir.to_str().unwrap())).expect("heredoc_body_to_fd");
    let link = std::fs::read_link(format!("/proc/self/fd/{fd}")).expect("readlink");
    let link = link.to_string_lossy().into_owned();
    assert!(link.starts_with(dir.to_str().unwrap()), "TMPDIR not honored: {link}");
    assert!(link.ends_with("(deleted)"), "temp file not unlinked: {link}");
    unsafe { libc::close(fd) };
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unusable_tmpdir_falls_back_to_tmp() {
    // bash silently falls back to /tmp when TMPDIR is unusable — verified with
    // both TMPDIR=/nonexistent/xx and TMPDIR=/proc (unwritable): rc 0, no
    // diagnostic, file lands in /tmp.
    let b = body(HEREDOC_PIPESIZE + 1);
    let fd = heredoc_body_to_fd(&b, Some("/nonexistent/xx")).expect("must fall back to /tmp");
    assert_eq!(fd_kind(fd), libc::S_IFREG);
    assert_eq!(read_all(fd), b);
}
```

Register the module in `crates/huck-engine/src/executor.rs`, appended to the `#[cfg(test)] mod …;` list that ends around `:8485`:

```rust
#[cfg(test)]
mod heredoc_body_tests;
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p huck-engine --jobs 1 --lib heredoc_body -- --test-threads 1
```
Expected: FAIL to compile — ``cannot find function `heredoc_body_to_fd` in module `super` `` and ``cannot find value `HEREDOC_PIPESIZE` ``.

- [ ] **Step 3: Write the implementation**

In `crates/huck-engine/src/executor.rs`, immediately ABOVE the existing `fn spawn_heredoc_writer` (`:3510`), add:

```rust
/// bash's `HEREDOC_PIPESIZE` (redir.c). A body at or below this goes to a pipe,
/// a larger one to a temp file. 65536 is the Linux default pipe capacity, which
/// is precisely why bash can write the pipe case from the parent without ever
/// blocking. Verified against bash 5.2.21: a 65536-byte body yields a pipe, a
/// 65537-byte body yields `/tmp/sh-thd.XXXXXX (deleted)`.
const HEREDOC_PIPESIZE: usize = 65536;

/// Deliver an expanded heredoc/here-string body and return a fresh READ-ONLY fd
/// positioned at offset 0, with the body ALREADY fully delivered — no forked
/// writer, matching bash's `here_document_to_fd`.
///
/// This is what makes #169 unreachable: a permanent (`exec`) redirect has no
/// reader until a LATER command runs, so any writer process still blocked on a
/// full pipe could never be reaped. With no writer, there is nothing to wait on.
///
/// `bytes.len()` is bash's `herelen` — here-string callers append the trailing
/// newline BEFORE calling, so it is included in the size decision.
///
/// `tmpdir` is the shell's `$TMPDIR` variable (NOT the process env: bash honors
/// an in-shell `TMPDIR=/x` whether exported or not, and huck does not sync
/// exports to the process env). An unusable value silently falls back to `/tmp`,
/// as bash does.
///
/// The caller owns the returned fd (and typically hands it to
/// `relocate_high_cloexec`). The contract is only "a fresh readable fd at offset
/// 0" — true of a pipe read end and a rewound file alike — so no call site needs
/// to know which path produced it.
fn heredoc_body_to_fd(bytes: &[u8], tmpdir: Option<&str>) -> Result<RawFd, io::Error> {
    // Size check FIRST so a large body does no wasted pipe work (bash's exact
    // behavior on Linux). The nonblocking probe inside `heredoc_body_to_pipe` is
    // the portability guard: bash hardcodes 65536 and writes BLOCKING, which is
    // safe only where a pipe holds 64KB. On macOS pipes start at 16KB, so that
    // same code has nothing to stop it wedging — we degrade to a temp file
    // instead of inheriting the hang (cf. #97, already a macOS-only hang).
    if bytes.len() <= HEREDOC_PIPESIZE {
        if let Some(fd) = heredoc_body_to_pipe(bytes) {
            return Ok(fd);
        }
    }
    heredoc_body_to_file(bytes, tmpdir)
}

/// Try to deliver `bytes` entirely into a pipe buffer, returning the read end.
/// `None` means "did not fit / could not" — the caller falls back to a temp file.
/// Never blocks: the write end is O_NONBLOCK, which is a property of THIS open
/// file description, so the reader's end (a distinct description) stays blocking
/// and the probe is invisible downstream.
fn heredoc_body_to_pipe(bytes: &[u8]) -> Option<RawFd> {
    let (r, w) = crate::child_fd::make_pipe(false).ok()?;
    // SAFETY: `r`/`w` are freshly-opened fds owned by us; every path below closes
    // both or returns `r` to the caller.
    unsafe {
        let fl = libc::fcntl(w, libc::F_GETFL);
        if fl < 0 || libc::fcntl(w, libc::F_SETFL, fl | libc::O_NONBLOCK) < 0 {
            libc::close(r);
            libc::close(w);
            return None;
        }
    }
    let mut off = 0usize;
    while off < bytes.len() {
        // SAFETY: writing from a live slice into an open fd.
        let n = unsafe {
            libc::write(
                w,
                bytes[off..].as_ptr() as *const libc::c_void,
                bytes.len() - off,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            // EAGAIN: this platform's pipe is smaller than the body. Anything
            // else: let the temp-file path have a go. Either way, discard.
            unsafe {
                libc::close(r);
                libc::close(w);
            }
            return None;
        }
        if n == 0 {
            unsafe {
                libc::close(r);
                libc::close(w);
            }
            return None;
        }
        off += n as usize;
    }
    // Close the write end so the reader sees EOF after the body. An empty body
    // lands here directly — a pipe that is immediately at EOF.
    unsafe { libc::close(w) };
    Some(r)
}

/// Spool `bytes` to an unlinked temp file and return a read-only fd at offset 0.
/// Tries `$TMPDIR` then `/tmp`, mirroring bash's silent fallback for an unset or
/// unusable `TMPDIR`.
fn heredoc_body_to_file(bytes: &[u8], tmpdir: Option<&str>) -> Result<RawFd, io::Error> {
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(d) = tmpdir {
        if !d.is_empty() {
            candidates.push(d);
        }
    }
    if !candidates.contains(&"/tmp") {
        candidates.push("/tmp");
    }
    let mut last_err: Option<io::Error> = None;
    for dir in candidates {
        match heredoc_body_to_file_in(bytes, dir) {
            Ok(fd) => return Ok(fd),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::from_raw_os_error(libc::ENOENT)))
}

/// One `mkstemp`-in-`dir` attempt. Follows bash's deliberate, race-conscious
/// order: open the read-only fd BEFORE closing the writable one, and only then
/// unlink — so nothing can substitute the name in between. `mkstemp` creates the
/// file 0600 (owner-only) and the unlink makes it unreachable by name at once,
/// so it also cannot survive a crash.
fn heredoc_body_to_file_in(bytes: &[u8], dir: &str) -> Result<RawFd, io::Error> {
    let mut tmpl: Vec<u8> = Vec::with_capacity(dir.len() + 16);
    tmpl.extend_from_slice(dir.as_bytes());
    if !tmpl.ends_with(b"/") {
        tmpl.push(b'/');
    }
    tmpl.extend_from_slice(b"sh-thd.XXXXXX\0");

    // SAFETY: `tmpl` is a NUL-terminated, writable buffer of the exact shape
    // mkstemp requires; it overwrites the trailing XXXXXX in place.
    let rw = unsafe { libc::mkstemp(tmpl.as_mut_ptr() as *mut libc::c_char) };
    if rw < 0 {
        return Err(io::Error::last_os_error());
    }
    let path = tmpl.as_ptr() as *const libc::c_char;

    let mut off = 0usize;
    while off < bytes.len() {
        // SAFETY: writing from a live slice into an open fd.
        let n = unsafe {
            libc::write(
                rw,
                bytes[off..].as_ptr() as *const libc::c_void,
                bytes.len() - off,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            unsafe {
                libc::unlink(path);
                libc::close(rw);
            }
            return Err(e);
        }
        if n == 0 {
            unsafe {
                libc::unlink(path);
                libc::close(rw);
            }
            return Err(io::Error::from_raw_os_error(libc::ENOSPC));
        }
        off += n as usize;
    }

    // bash's order: second fd opened before the first is closed, then unlink.
    // The fresh O_RDONLY fd starts at offset 0 — no lseek needed.
    let ro = unsafe { libc::open(path, libc::O_RDONLY) };
    let err = if ro < 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    unsafe {
        libc::unlink(path);
        libc::close(rw);
    }
    match err {
        Some(e) => Err(e),
        None => Ok(ro),
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib heredoc_body -- --test-threads 1
```
Expected: PASS, 9 tests (`ok. 9 passed`). `spawn_heredoc_writer` is still live and wired, so nothing else changes.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/executor.rs crates/huck-engine/src/executor/heredoc_body_tests.rs
git commit -m "$(cat <<'EOF'
v307: heredoc_body_to_fd — pipe-or-tempfile body delivery, no forked writer (#169)

Adds bash's here_document_to_fd model: a pipe written directly by the parent
for bodies <= HEREDOC_PIPESIZE (65536), an unlinked temp file above it. Not
wired to any call site yet — spawn_heredoc_writer is still in use.

The fd is read-only on both paths and starts at offset 0, matching bash
(verified on 5.2.21: fdinfo access mode O_RDONLY; writing gives EBADF).
The temp-file path follows bash's race-conscious order — reopen O_RDONLY
before closing the writable fd, then unlink.

Unlike bash, the pipe write is O_NONBLOCK with a temp-file fallback: bash's
blocking write is safe only where a pipe holds 64KB, and macOS pipes start
at 16KB (cf. #97). O_NONBLOCK is per open-file-description, so the reader's
end stays blocking.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Wire it in — the #169 hang is fixed here

Swaps all six `spawn_heredoc_writer` call sites to the new helper and deletes the old function. The `heredoc_writers` vectors survive this task but are never pushed to, so every reap loop becomes a no-op over an empty vec and the tree keeps compiling. **This is the task that fixes the bug**; Task 3 is pure subtraction.

**Files:**
- Create: `tests/scripts/heredoc_exec_diff_check.sh`
- Modify: `crates/huck-engine/src/executor.rs` — call sites at `:5047`, `:5070` (`lower_one_redirect`, in-process arms), `:5265`, `:5296` (`lower_one_redirect`, plan arms), `:6457`, `:6501` (pipeline `spawn_pipeline` arms); delete `spawn_heredoc_writer` (`:3510`).

**Interfaces:**
- Consumes: `heredoc_body_to_fd(bytes: &[u8], tmpdir: Option<&str>) -> Result<RawFd, io::Error>` and `HEREDOC_PIPESIZE` from Task 1.
- Produces: no new API. After this task nothing pushes to any `heredoc_writers` vec — the precondition Task 3 relies on.

- [ ] **Step 1: Write the failing bash-diff harness**

Create `tests/scripts/heredoc_exec_diff_check.sh`. It is auto-discovered by `run_diff_checks.sh` (glob `tests/scripts/*_diff_check.sh`). Modelled on the existing `heredoc_redirect_fail_hang_diff_check.sh` (#142's guard), which uses the same `timeout` + `norm` idiom:

```bash
#!/usr/bin/env bash
# v307 (#169): `exec` with a heredoc/here-string body larger than the pipe
# buffer must NOT hang. huck used to install the heredoc pipe's read end on the
# target fd and then synchronously reap the forked writer — but a PERMANENT
# (`exec`) redirect has no reader until a LATER command, so a >64KB writer was
# still blocked on a full pipe and waitpid never returned.
#
# Every case wraps BOTH shells in `timeout` so a re-introduced HANG fails the
# gate (timeout -> mismatched output) instead of wedging CI.
#
# Deliberately NOT tested here: which fd TYPE the body lands on. bash uses a
# pipe <=64KB and an unlinked temp file above it, but the path differs per
# process, so `readlink /proc/$$/fd/3` could never be byte-identical. That check
# lives in the unit layer (executor/heredoc_body_tests.rs) via fstat.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

# Bodies straddling bash's 65536-byte HEREDOC_PIPESIZE boundary. Kept as shell
# fragments so both shells build them identically.
BIG='"$(head -c 70000 /dev/zero | tr "\0" x)"'
AT='"$(head -c 65535 /dev/zero | tr "\0" x)"'    # +newline = 65536 -> pipe
OVER='"$(head -c 65536 /dev/zero | tr "\0" x)"'  # +newline = 65537 -> temp file

FAIL=0
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
check() {
  local label=$1 frag=$2 b h
  b=$( { timeout 10 bash    -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { timeout 10 "$HUCK" -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"
    echo "  bash: $(printf '%s' "$b" | head -c 200)"
    echo "  huck: $(printf '%s' "$h" | head -c 200)"
    FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- #169 proper: exec + big here-string, read by a LATER command.
check 'exec-herestring-rc'     "exec 3<<<$BIG; echo rc=\$?"
check 'exec-herestring-head'   "exec 3<<<$BIG; head -c5 <&3"
check 'exec-herestring-wc'     "exec 3<<<$BIG; wc -c <&3"
# --- the same via a heredoc body rather than a here-string.
check 'exec-heredoc-wc'        "V=\$(head -c 70000 /dev/zero | tr '\0' x); exec 3<<EOF
\$V
EOF
wc -c <&3"
# --- boundary: at (pipe) and just over (temp file) bash's 65536 herelen.
check 'exec-at-boundary'       "exec 3<<<$AT; wc -c <&3"
check 'exec-over-boundary'     "exec 3<<<$OVER; wc -c <&3"
# --- a small body must keep working (pipe path, no regression).
check 'exec-small'             "exec 3<<<hi; cat <&3"
# --- the heredoc fd is READ-ONLY in bash, at both sizes: "Bad file descriptor".
check 'exec-fd-ro-small'       "exec 3<<<hi; echo x >&3; echo rc=\$?"
check 'exec-fd-ro-big'         "exec 3<<<$BIG; echo x >&3; echo rc=\$?"
# --- several exec heredocs live in one shell at once.
check 'exec-multi-fd'          "exec 3<<<$BIG; exec 4<<<$BIG; head -c3 <&3; head -c3 <&4; echo"
# --- exec heredoc + a partial read, then the shell exits (no lingering writer).
check 'exec-partial-read'      "exec 3<<<$BIG; head -c5 <&3; echo done"

if [ $FAIL -ne 0 ]; then echo "heredoc_exec_diff_check FAILED" >&2; exit 1; fi
echo "heredoc_exec_diff_check OK"
```

- [ ] **Step 2: Run the harness to verify it fails**

```bash
cargo build -p huck
bash tests/scripts/heredoc_exec_diff_check.sh
```
Expected: FAIL. The `exec-*` big-body cases hang huck until `timeout 10` kills it, so huck's output is empty with `rc=124` while bash returns the body and `rc=0`. `exec-small` and the boundary `exec-at-boundary` case should already PASS (they fit the pipe).

- [ ] **Step 3: Swap the call sites**

All six sites lose their `writers.push(pid)` / `match mode { … }` bookkeeping. Read `$TMPDIR` from the SHELL, not the process env.

**(a) `crates/huck-engine/src/executor.rs:5045` — `lower_one_redirect`, in-process `Heredoc` arm.** Replace:

```rust
            RedirOp::Heredoc { body, .. } => {
                let bytes = expand_assignment(body, shell).into_bytes();
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        writers.push(pid);
                        owned_src = Some(unsafe { OwnedFd::from_raw_fd(rfd) });
                    }
```
with:
```rust
            RedirOp::Heredoc { body, .. } => {
                let bytes = expand_assignment(body, shell).into_bytes();
                let tmpdir = shell.lookup_var("TMPDIR");
                match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                    Ok(rfd) => {
                        owned_src = Some(unsafe { OwnedFd::from_raw_fd(rfd) });
                    }
```
The `Err(e) => { … "heredoc: {}", crate::bash_io_error(&e) … return Err(1); }` arm is UNCHANGED — same diagnostic, same `Err(1)`.

**(b) `:5068` — `lower_one_redirect`, in-process `HereString` arm.** Same shape; the `bytes.push(b'\n')` line stays (it is what makes `bytes.len()` bash's `herelen`):

```rust
            RedirOp::HereString(w) => {
                let mut bytes = expand_assignment(w, shell).into_bytes();
                bytes.push(b'\n');
                let tmpdir = shell.lookup_var("TMPDIR");
                match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                    Ok(rfd) => {
                        owned_src = Some(unsafe { OwnedFd::from_raw_fd(rfd) });
                    }
```

**(c) `:5263` — `lower_one_redirect`, plan `Heredoc` arm.** Keep `relocate_high_cloexec`:

```rust
        RedirOp::Heredoc { body, .. } => {
            let bytes = expand_assignment(body, shell).into_bytes();
            let tmpdir = shell.lookup_var("TMPDIR");
            match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                Ok(rfd) => {
                    let rfd = relocate_high_cloexec(rfd);
                    let owned = unsafe { OwnedFd::from_raw_fd(rfd) };
                    ops.push(PlanOp::InstallOwned {
                        target,
                        source: owned,
                    });
                    if let Some(st) = fd_state.as_deref_mut() {
                        st.insert(target, true);
                    }
                }
```

**(d) `:5294` — `lower_one_redirect`, plan `HereString` arm.** Same as (c), retaining `bytes.push(b'\n')`.

**(e) `:6456` — `spawn_pipeline`, `Heredoc` arm.** Replace the writer-mode bookkeeping. The comment above it narrates the forked writer and must be rewritten:

```rust
                    // Expand the body NOW while inline assignments are still applied,
                    // then deliver it (pipe or temp file — no forked writer, #169).
                    let bytes = expand_assignment(body, shell).into_bytes();
                    let tmpdir = shell.lookup_var("TMPDIR");
                    match heredoc_body_to_fd(&bytes, tmpdir.as_deref()) {
                        Ok(r) => unsafe { ChildFd::owned_raw(r) },
```
The `SpawnMode::Foreground`/`SpawnMode::Background` match goes away entirely — with no pid there is nothing to route. (`mode` stays in use elsewhere in this function; if the compiler reports it newly unused here, that is expected and fine.)

**(f) `:6500` — `spawn_pipeline`, `HereString` arm.** Same as (e), retaining `bytes.push(b'\n')`.

**(g) Delete `fn spawn_heredoc_writer`** entirely (`:3510` through its closing brace, including its doc comment). It now has no callers; leaving it would be dead code.

- [ ] **Step 4: Run the harness to verify it passes**

```bash
cargo fmt --all
cargo build -p huck
bash tests/scripts/heredoc_exec_diff_check.sh
```
Expected: `heredoc_exec_diff_check OK`, all 11 cases PASS.

- [ ] **Step 5: Verify the #142 guard and the four-path guard still pass**

The spec commits to keeping #142's regression test passing on BEHAVIOR, not mechanism — it is the check that removing the restore-then-reap ordering did not resurrect that hang.

```bash
bash tests/scripts/heredoc_redirect_fail_hang_diff_check.sh
bash tests/scripts/heredoc_pipeline_diff_check.sh
bash tests/scripts/heredoc_redir_v266_diff_check.sh
bash tests/scripts/fd_redirect_diff_check.sh
cargo test -p huck --test heredoc_forked_writer_integration --jobs 1 -- --test-threads 1
cargo test -p huck --test heredoc_integration --jobs 1 -- --test-threads 1
cargo test -p huck --test here_string_integration --jobs 1 -- --test-threads 1
```
Expected: all OK / `test result: ok`. `heredoc_forked_writer_integration` is the load-bearing one: it drives 200KB bodies (→ temp-file path) through the compound, pipeline, and captured paths and asserts `$!` is unaffected — i.e. it already covers three of the spec's four paths.

If any FAIL, stop and report rather than patching around it.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/executor.rs tests/scripts/heredoc_exec_diff_check.sh
git commit -m "$(cat <<'EOF'
v307: fix exec heredoc hang — wire heredoc_body_to_fd, drop the forked writer (#169)

Swaps all six spawn_heredoc_writer call sites (the two in-process
lower_one_redirect arms, the two plan arms, and the two pipeline arms) to
heredoc_body_to_fd, and deletes spawn_heredoc_writer.

`exec 3<<<BIG` no longer hangs: with no writer process, the permanent
redirect has nothing to waitpid on. Guarded by heredoc_exec_diff_check.sh,
which wraps both shells in `timeout` so a regression fails the gate rather
than wedging CI.

$TMPDIR is read from the SHELL variable, not the process env — bash honors
an in-shell TMPDIR=/x whether exported or not, and huck does not sync
exports to the process env.

The heredoc_writers vectors still exist but now have no producer; they are
deleted in the next commit.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Delete the dead `heredoc_writers` plumbing

Pure subtraction — no behavior change. After Task 2 nothing pushes to these vectors, so every reap loop is a no-op over an empty vec. A reviewer can reject this task while keeping Task 2's fix.

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (all sites below)
- Rename: `tests/heredoc_forked_writer_integration.rs` → `tests/heredoc_large_body_integration.rs`

**Interfaces:**
- Consumes: Task 2's guarantee that no producer remains.
- Produces: `lower_one_redirect` loses its `writers` parameter — final signature:
  ```rust
  fn lower_one_redirect(
      redir: &Redirection,
      shell: &mut Shell,
      sink: &mut StdoutSink,
      err_sink: &mut StderrSink,
      mut fd_state: Option<&mut std::collections::HashMap<RawFd, bool>>,
  ) -> Result<Vec<PlanOp>, i32>
  ```
  `RedirPlan`, `ChildRedirPlan`, and `SpawnedPipeline` each lose their `heredoc_writers` field.

- [ ] **Step 1: Delete the `RedirectScope` half**

In `crates/huck-engine/src/executor.rs`:
- `:970` — remove the `heredoc_writers: Vec<libc::pid_t>,` field from `struct RedirectScope`, and its `heredoc_writers: Vec::new(),` initializer in `RedirectScope::new` (`:977`).
- `:1115` — delete `fn reap_heredoc_writers` entirely.
- `:1152` — in `impl Drop for RedirectScope`, delete the trailing `self.reap_heredoc_writers();` call **and** the `#142` comment block above it explaining restore-then-reap. Replace that comment with:
  ```rust
        // #169/v307: there is nothing to reap here any more. Heredoc bodies are
        // delivered by `heredoc_body_to_fd` (pipe or temp file) with no forked
        // writer, so the restore-then-reap ordering #142 needed — restore first so
        // a writer blocked on a full pipe gets EPIPE and can be waited on — is
        // moot. The guard remains in heredoc_redirect_fail_hang_diff_check.sh.
  ```
- `:1101` — in `RedirectScope::apply_redirects`, drop the `&mut self.heredoc_writers,` argument from the `lower_one_redirect(...)` call.
- Update the `struct RedirectScope` doc comment (`:955-967`): delete the final sentence, "Heredoc/here-string writer pids spawned during resolution are tracked in `heredoc_writers` and reaped by `reap_heredoc_writers` after the body has run."

- [ ] **Step 2: Delete the `lower_one_redirect` parameter and the plan halves**

- `:4979` — remove the `writers: &mut Vec<libc::pid_t>,` parameter. In its doc comment (`:4968`), change "opens files (as OwnedFd), spawns heredoc writers (pushing the writer pid onto `writers`)" to "opens files (as OwnedFd), delivers heredoc bodies via `heredoc_body_to_fd`".
- `:4903` — remove `heredoc_writers: Vec<libc::pid_t>,` from `struct RedirPlan`, and update its doc comment (`:4896-4902`).
- `:4953` — remove `heredoc_writers: Vec<libc::pid_t>,` from `struct ChildRedirPlan`, and drop the sentence "`heredoc_writers` are forked body writers to reap after the child finishes." from its doc comment.
- `:5365` / `:5374` / `:5381` — in `lower_redirects`: drop the `heredoc_writers: Vec::new(),` initializer, drop the `&mut plan.heredoc_writers,` argument, and in the `Err(code)` arm delete the reap loop, leaving:
  ```rust
              Err(code) => {
                  // Dropping `plan.ops` closes every fd opened so far (heredoc
                  // read ends included). No writers exist to reap (#169).
                  plan.ops.clear();
                  restore(shell, var_snaps);
                  return Err(code);
              }
  ```
- `:5417` — in `redir_plan_to_child`, drop `heredoc_writers: plan.heredoc_writers,` from the `ChildRedirPlan` construction.

- [ ] **Step 3: Delete the subprocess and pipeline halves**

- `:5625` — in `run_subprocess`, delete `let heredoc_writers = plan.heredoc_writers;` and fix the comment above it (`:5622-5624`), which claims heredoc read-ends close "once the child exits" via the writer. Then delete the three reap loops at `:5781`, `:5791`, `:5809` (the `Ok(mut child)` success arm and both spawn-failure arms), plus the "The heredoc/herestring body (if any) is written by the forked writer process…" comment at `:5628`.
- `:5851` — remove `heredoc_writers: Vec<i32>,` from `struct SpawnedPipeline`.
- `:6142` — in `spawn_pipeline`, delete `let mut heredoc_writers: Vec<libc::pid_t> = Vec::new();` and the four-line `M-120` comment above it.
- `:6631` — in the `build_child_redir_plan` arm, collapse the mode match:
  ```rust
                  match build_child_redir_plan(&exec.redirects, shell, sink, err_sink) {
                      Ok(p) => Some(p),
  ```
- `:6970` — drop `heredoc_writers,` from the `SpawnedPipeline { … }` construction.
- `:6991` — in `run_multi_stage`'s destructuring of `SpawnedPipeline`, drop the `heredoc_writers,` binding. Fix that function's doc comment (`:6978-6980`), which says "…wait for all stages (setting `$PIPESTATUS`), reap the heredoc writers, and drain process substitutions" — remove the reap clause.
- `:7080` — delete the final reap loop and its `M-120` comment.

- [ ] **Step 4: Retitle the four-path guard**

The test is behavioral and still valid — only its name and header narrate the deleted mechanism.

```bash
git mv tests/heredoc_forked_writer_integration.rs tests/heredoc_large_body_integration.rs
```
Replace the header (first line) of `tests/heredoc_large_body_integration.rs`:
```rust
//! Large heredoc/herestring bodies (> pipe buffer) and backpressuring consumers
//! must not deadlock, on every exec path: compound, pipeline, and captured.
//! v134 achieved this with a forked writer (M-120); v307 (#169) replaced that
//! with bash's pipe-or-tempfile delivery (`heredoc_body_to_fd`). The behavior
//! asserted here is mechanism-agnostic and must hold either way.
```

- [ ] **Step 5: Verify it still compiles clean and every guard passes**

```bash
cargo fmt --all
cargo build -p huck 2>&1 | grep -E "^(warning|error)" || echo "no warnings/errors"
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
bash tests/scripts/heredoc_exec_diff_check.sh
bash tests/scripts/heredoc_redirect_fail_hang_diff_check.sh
cargo test -p huck --test heredoc_large_body_integration --jobs 1 -- --test-threads 1
cargo test -p huck --test heredoc_integration --jobs 1 -- --test-threads 1
```
Expected: no `dead_code` warnings (a leftover unused field or fn means a site was missed), engine lib tests `ok` (~1773 + the 9 new), both harnesses OK, both integration binaries `ok`.

Note: CI does NOT use `-D warnings`, so a missed deletion will NOT fail the build — grep the build output yourself.

- [ ] **Step 6: Commit**

```bash
git add -A crates/huck-engine/src/executor.rs tests/
git commit -m "$(cat <<'EOF'
v307: delete the now-dead heredoc_writers plumbing (#169)

With heredoc_body_to_fd there is no forked writer, so the writer-pid
plumbing has no producer: the RedirectScope field + reap_heredoc_writers +
the Drop reap, lower_one_redirect's `writers` param, RedirPlan /
ChildRedirPlan / SpawnedPipeline fields, and the six reap loops across
run_subprocess, lower_redirects, and the pipeline paths.

This retires #142's restore-then-reap invariant in RedirectScope::Drop.
Not a regression: #142's hang WAS a blocked writer, so removing writers
removes the failure mode the ordering defended against. Its regression
test (heredoc_redirect_fail_hang_diff_check.sh) is kept and still passes.

Also removes the Foreground/Background asymmetry where background writers
were silently left to the SIGCHLD reaper, and a class of stray SIGCHLD from
children that were "not jobs, not $!".

heredoc_forked_writer_integration.rs -> heredoc_large_body_integration.rs:
the assertions are mechanism-agnostic (200KB bodies through the compound,
pipeline, and captured paths), only the name narrated the old mechanism.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Full verification, docs, follow-on issue, PR

**Files:**
- Modify: `docs/architecture.md` (only if it names `spawn_heredoc_writer` or the writer model)
- Modify: memory files `project_huck_iterations.md` + `MEMORY.md`

**Interfaces:**
- Consumes: a green tree from Tasks 1-3.
- Produces: PR against `main` with `Closes #169`, left for the USER to merge.

- [ ] **Step 1: Sweep the docs for the deleted mechanism**

```bash
grep -rn "spawn_heredoc_writer\|heredoc_writers\|forked writer" docs/ README.md
```
Expected: no live references outside `docs/superpowers/specs/` and `docs/superpowers/plans/` (historical paper trail — leave those alone). Update `docs/architecture.md` if it describes the writer model. Prior iterations have been bitten by a doc naming a removed API, so this grep is not optional.

- [ ] **Step 2: Run the full local test suite, per-crate**

```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```
Expected: `ok` for both (~441 and ~1782).

- [ ] **Step 3: Run the `-p huck` integration binaries individually**

`--lib` does NOT cover these, and they DO run in CI — v289 passed `--lib` + the sweep and still failed CI. Run the redirect/heredoc/job-adjacent ones at minimum:

```bash
for t in heredoc_large_body_integration heredoc_integration here_string_integration \
         compound_redirects_integration fd_dup_integration named_fd_integration \
         external_fd_redirects_integration function_redirect_integration \
         builtin_fd_ordering_integration noclobber_integration io_error_integration \
         captured_pipeline_drain_integration pipeline_subshell_integration \
         subshell_pipeline_integration sigpipe_integration wait_integration; do
  echo "=== $t"
  (ulimit -v 1500000; timeout 300 cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1) \
    || echo "!!! FAILED: $t"
done
```
Expected: `test result: ok` for each; no `!!! FAILED` lines.

- [ ] **Step 4: Run the differential fd gate**

`tools/redirect_audit.sh` is the standing gate for ANY fd change — a green diff sweep has lied before on exactly this class of work.

```bash
(ulimit -v 1500000; timeout 600 bash tools/redirect_audit.sh) 2>&1 | tail -20
(ulimit -v 1500000; timeout 600 bash tools/pipeline_redirect_audit.sh) 2>&1 | tail -20
(ulimit -v 1500000; timeout 600 bash tools/bg_pipeline_redirect_audit.sh) 2>&1 | tail -20
```
Expected: no new divergences vs `main`. If a case regresses, STOP and report — do not paper over it.

- [ ] **Step 5: Run the full bash-diff sweep**

```bash
cargo build --locked --bin huck
cargo build --release --locked --bin huck
(ulimit -v 1500000; timeout 1800 tests/scripts/run_diff_checks.sh) 2>&1 | tail -25
```
Expected: green, EXCEPT the known pre-existing v296 flake [#180](https://github.com/jdstanhope/huck/issues/180) (`PIPESTATUS[0]` race). Any OTHER failure is a regression — stop and report. Re-run a single failure before calling it a flake.

- [ ] **Step 6: Open the follow-on divergence issue**

Discovered while designing this: `crates/huck-engine/src/procsub.rs:133` reads `TMPDIR` via `std::env::var("TMPDIR")`, which misses an in-shell `TMPDIR=/x` (huck does not sync exports to the process env). bash honors the shell variable — verified on 5.2.21 for heredocs, and process substitution uses the same tmpdir logic. Out of scope for #169; CLAUDE.md requires a new issue for any follow-on gap found.

```bash
gh issue create --label divergence --label bug --label sev:low \
  --title 'process substitution reads TMPDIR from the process env, not the shell variable' \
  --body "$(cat <<'EOF'
`crates/huck-engine/src/procsub.rs:133` resolves its temp directory with
`std::env::var("TMPDIR")`, which sees only the INHERITED process environment.
huck does not sync exported shell variables to the process env, so an in-shell
`export TMPDIR=/x` (or a plain `TMPDIR=/x`) is ignored.

bash honors the shell VARIABLE, exported or not — verified on 5.2.21 with the
heredoc temp-file path, which shares the same tmpdir logic:

```
$ bash -c 'export TMPDIR=/tmp/mytd2; exec 3<<<"$(head -c 70000 /dev/zero | tr "\0" x)"; readlink /proc/$$/fd/3'
/tmp/mytd2/sh-thd.wcUlHJ (deleted)
$ bash -c 'TMPDIR=/tmp/mytd2; exec 3<<<"$(head -c 70000 /dev/zero | tr "\0" x)"; readlink /proc/$$/fd/3'
/tmp/mytd2/sh-thd.8h7uW1 (deleted)
```

Fix shape: resolve via `shell.lookup_var("TMPDIR")`, as v307's
`heredoc_body_to_fd` does (#169).

Found while designing v307 / #169.
EOF
)"
```
Record the issue number it prints — it goes in the PR body as a follow-on note.

- [ ] **Step 7: Record the iteration in memory**

Append a v307 entry to `project_huck_iterations.md` (newest at top) covering: #169 fixed by adopting bash's pipe-or-tempfile model; `spawn_heredoc_writer` and the whole `heredoc_writers` plumbing deleted; #142's restore-then-reap invariant retired as moot (guard kept); the durable lesson that **bash's own 65536 constant is load-bearing on Linux's pipe capacity and is not portable** — huck's nonblocking probe is deliberately safer than bash here; and the follow-on `TMPDIR` issue from Step 6.

Update the `MEMORY.md` iteration-log hook to `**Latest: v307**` with a ONE-line summary. Do not inline the detail into `MEMORY.md`.

- [ ] **Step 8: Push and open the PR — do NOT merge**

```bash
git push -u origin v307-exec-heredoc-tempfile
gh pr create --base main --title 'v307: exec heredoc hang — bash pipe-or-tempfile body delivery (#169)' --body "$(cat <<'EOF'
Closes #169.

`exec 3<<<BIG` (a heredoc/here-string body over the ~64KB pipe buffer) hung
huck: `apply_redirects_permanently` installed the pipe's read end on the target
fd and then synchronously reaped the forked writer — but a PERMANENT redirect's
reader is a LATER command, so the writer was still blocked on a full pipe and
`waitpid` never returned.

Rather than special-case `exec`, this adopts bash's model wholesale. bash has no
writer to reap at any size, which is why it cannot hit this:

| body length | bash mechanism |
| --- | --- |
| <= 65536 bytes | pipe, written directly by the parent, no fork |
| > 65536 bytes | unlinked temp file, rewound to offset 0 |

`spawn_heredoc_writer` becomes `heredoc_body_to_fd`, and the hang stops being
reachable by construction — with no writer, there is nothing to wait on. The
`heredoc_writers` plumbing (RedirectScope, RedirPlan, ChildRedirPlan,
SpawnedPipeline, six reap loops) loses its only producer and is deleted, which
collapses a mechanism that was duplicated across the fg-pipeline, bg, subshell,
and capture paths onto one helper.

### bash 5.2.21 behavior, verified rather than assumed

- The 65536 boundary (65535+newline -> pipe; 65536+newline -> temp file).
- **The heredoc fd is READ-ONLY** at both sizes (`fdinfo` access mode
  `O_RDONLY`; `echo x >&3` gives `Bad file descriptor`). The issue text did not
  mention this; missing it would have been a silent divergence.
- The reopen-O_RDONLY-before-unlink-and-close ordering.
- `TMPDIR` is honored as a SHELL variable, exported or not — so this reads
  `shell.lookup_var("TMPDIR")`, not `std::env::var`.

### One deliberate divergence from bash

bash writes the pipe case with a BLOCKING write, safe only because 65536 is
Linux's default pipe capacity. On macOS, where pipes start at 16KB, that same
code has nothing to stop it wedging. huck does the size check first (bash's
exact behavior on Linux, no wasted work) and then probes with `O_NONBLOCK`,
falling back to a temp file if the body does not fit. Given #97 is already a
macOS-only hang, this avoids planting another one.

### #142

This retires #142's restore-then-reap invariant in `RedirectScope::Drop` as
moot — that hang WAS a blocked writer. Its regression test
(`heredoc_redirect_fail_hang_diff_check.sh`) is KEPT and still passes, on
behavior rather than mechanism.

### Verification

- New `tests/scripts/heredoc_exec_diff_check.sh` (11 cases; both shells wrapped
  in `timeout` so a regression FAILS the gate instead of wedging CI).
- New unit tests pin path SELECTION via `fstat` — which a diff harness
  structurally cannot do, since the temp path differs per process.
- `heredoc_forked_writer_integration` -> `heredoc_large_body_integration`,
  unchanged assertions: 200KB bodies through the compound, pipeline, and
  captured paths, plus `$!`.
- `tools/redirect_audit.sh` + the pipeline/bg variants: no new divergences.
- Full bash-diff sweep green apart from the known #180 flake.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Then hand the PR URL to the user. **Do not merge it.**

---

## Notes for the implementer

- **`mode` in `spawn_pipeline`**: Task 2(e)/(f) delete a `match mode { Foreground … Background … }`. `mode` is used elsewhere in that function (the stage-0 stdin default at `:6144`), so it should not become unused — but if the compiler says otherwise, that is a signal you deleted more than intended.
- **Borrow order**: `expand_assignment(body, shell)` takes `&mut shell`, so bind `let tmpdir = shell.lookup_var("TMPDIR");` on the NEXT line and pass `tmpdir.as_deref()`. Do not inline the lookup into the `heredoc_body_to_fd(...)` call while `bytes` is still borrowing.
- **`lookup_var` returns `Option<String>`** (see `:4989`'s `shell.lookup_var(name).unwrap_or_default()`).
- **Do not "improve" the error path.** Both paths keep the existing `heredoc: <errno>` diagnostic and `Err(1)`. bash's temp-file error wording could not be provoked (it falls back to `/tmp` even from `TMPDIR=/proc`), and inventing an unverifiable message is how prior error-text divergences were created.
- **If a bash-diff case disagrees, verify against real `bash 5.2.21` before "fixing" huck.** An issue's suggested fix has been wrong before (#302's proposed exit-status decode).
