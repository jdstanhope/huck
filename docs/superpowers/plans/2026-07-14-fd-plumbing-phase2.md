# Implementation plan — v291 / fd-plumbing Phase 2: universal fd hygiene at creation

**Spec:** `docs/superpowers/specs/2026-07-14-fd-plumbing-phase2-design.md`
(committed on `main` @ `41498ad`).

## Goal

Consolidate the engine's fd-creation hygiene into single helpers, and in doing
so close [#135](https://github.com/jdstanhope/huck/issues/135) (an in-process
whole-command redirect whose opened file lands on a freed 0/1/2 vanishes on the
child's exec). Three refactors: fold the three high-fd helpers into one
parameterized pair (T1), route every redirect File-open through one relocating
`open_redirect_file` including the in-process sites (T2, the #135 fix), and
unify the two production `make_pipe` implementations plus the raw `libc::pipe`
sites behind one `make_pipe(cloexec)` (T3).

## Architecture

The engine forks/spawns children through several launch paths, each of which
today re-derives child fds by hand and creates internal fds with one of three
high-fd relocation helpers (two thresholds, three CLOEXEC policies) and one of
two production `make_pipe`s plus two raw `libc::pipe` sites. Phase 1 (v290)
introduced `child_fd.rs` with `ChildFd`/`ChildStdio`; Phase 2 moves fd-creation
hygiene (relocation + pipe creation) into that same module and makes redirect
file-opens uniformly relocate ≥10 + CLOEXEC. The redirect *lowering*
orchestration and the software-sink (capture/merge) layer are deliberately
untouched (Phase 3).

## Tech Stack

Rust (edition 2024), `libc` for `fcntl`/`dup2`/`pipe`/`close`,
`std::os::fd::{OwnedFd, RawFd, BorrowedFd}` for RAII fd ownership. Test harnesses
are bash byte-diff scripts (`tests/scripts/*_diff_check.sh`) plus `-p huck`
integration binaries under `tests/`.

## Global Constraints

- **Commit trailer** on every commit:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **`cargo fmt --all` before every commit** (CI enforces `cargo fmt --all
  --check`); the tree must be **warning-clean** (`dead_code` gets unmasked by
  deletions — remove now-unused helpers/imports in the same task).
- **OOM (this 1-core / 1.9 GB box):** NEVER `cargo test --workspace` (it
  OOM-kills the session). Use per-crate single-threaded
  `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`; build the
  binary with `cargo build -p huck`; run `-p huck` integration binaries one at a
  time single-threaded under `ulimit -v 6000000`
  (`ulimit -v 6000000; cargo test -p huck --test <name> --jobs 1 -- --test-threads 1`);
  run the diff sweep under `ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh`.
- **Issue:** the PR will `Closes #135`
  (https://github.com/jdstanhope/huck/issues/135).
  [#137](https://github.com/jdstanhope/huck/issues/137) (builtin write to a
  closed fd 1 swallowed) is an OUT-OF-SCOPE open bug — the `fd_torture` #135
  proof must NOT depend on it (route the `>&-` case through stderr).
- **Behavior-preserving EXCEPT closing #135.** `fd_torture_diff_check.sh` + the
  full `run_diff_checks.sh` sweep + the touched `-p huck` integration binaries
  are the regression net.
- **Line numbers drift on `a191f33`.** Function names + the before-snippets in
  this plan are the stable handles — **locate every edit by `grep` for the
  function name / snippet, not by line number.**

---

## Task 1 — Fold the high-fd helpers into `dup_to_high_fd` / `move_to_high_fd`

Spec §T1. Add the unified pair (+ move `set_cloexec`) to `child_fd.rs`; rewire
every caller of `alloc_high_fd` and `relocate_high_cloexec` and the
`make_pipe`/`move_fd_above_stdio` coupling.

**`make_pipe` coupling decision (explicit):** Task 1 **rewires the current
`make_pipe` to call `move_to_high_fd(fd, 3, false)` and DELETES
`move_fd_above_stdio` now.** (Task 3 later replaces `make_pipe`'s body wholesale
with the `cloexec`-parameterized version; deleting `move_fd_above_stdio` in T1
keeps T1 warning-clean without a lingering single-use helper.) T1's `make_pipe`
stays non-parameterized (`false` hardcoded via the `move_to_high_fd(fd, 3,
false)` calls); T3 adds the `cloexec` parameter.

### Step 1.1 — Add the helpers + `set_cloexec` to `child_fd.rs` (TDD)

Locate `child_fd.rs`. Add these imports if missing (it already imports
`AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd` and `io`):

Append to the impl area of `crates/huck-engine/src/child_fd.rs` (module-level
functions, after the `ChildStdio` impl, before `#[cfg(test)] mod tests;`):

```rust
/// Set FD_CLOEXEC on a raw fd so it does NOT leak into an exec'd program.
/// (Moved from executor.rs in Phase 2; used by the best-effort relocation
/// fallback and the macOS `make_pipe` path.)
pub(crate) fn set_cloexec(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
        }
    }
}

/// Dup `src` to the lowest free fd >= `min` (`F_DUPFD` / `F_DUPFD_CLOEXEC` per
/// `cloexec`). `src` is left OPEN (caller-owned) — this is the dup-not-move
/// primitive the `{var}` sites need (they keep the source and close it via
/// their own `owns_src` logic). Errors: EMFILE/EBADF.
pub(crate) fn dup_to_high_fd(src: RawFd, min: RawFd, cloexec: bool) -> io::Result<RawFd> {
    let cmd = if cloexec {
        libc::F_DUPFD_CLOEXEC
    } else {
        libc::F_DUPFD
    };
    let fd = unsafe { libc::fcntl(src, cmd, min) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// `dup_to_high_fd` + `close(src)`. Unconditional: relocates even when `src` is
/// already >= `min` (matches the old `relocate_high_cloexec`; the hot-path
/// `fd > 2` no-op conditional lives only in `make_pipe`). On Err, `src` is left
/// OPEN (caller cleans up). huck's analogue of bash's `move_to_high_fd`, but
/// with an explicit `min` + kernel lowest-free-≥min (one `F_DUPFD` syscall, no
/// downward scan): the thresholds (3, 10) are load-bearing and frozen.
pub(crate) fn move_to_high_fd(src: RawFd, min: RawFd, cloexec: bool) -> io::Result<RawFd> {
    let new = dup_to_high_fd(src, min, cloexec)?;
    unsafe {
        libc::close(src);
    }
    Ok(new)
}
```

Add unit tests. Append into `crates/huck-engine/src/child_fd/tests.rs` (it
already exists per Phase 1). Use FRESH fds only (a `/dev/null` open or a pipe);
never touch 0/1/2:

```rust
#[test]
fn dup_to_high_fd_keeps_src_open_and_honors_cloexec() {
    // Fresh source fd from /dev/null.
    let f = std::fs::File::open("/dev/null").unwrap();
    let src = f.into_raw_fd();

    // Non-CLOEXEC dup.
    let a = dup_to_high_fd(src, 10, false).unwrap();
    assert!(a >= 10);
    // src still open.
    assert!(unsafe { libc::fcntl(src, libc::F_GETFD) } >= 0);
    let flags = unsafe { libc::fcntl(a, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, 0);

    // CLOEXEC dup.
    let b = dup_to_high_fd(src, 10, true).unwrap();
    assert!(b >= 10);
    let flags = unsafe { libc::fcntl(b, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);

    unsafe {
        libc::close(a);
        libc::close(b);
        libc::close(src);
    }
}

#[test]
fn move_to_high_fd_closes_src() {
    let f = std::fs::File::open("/dev/null").unwrap();
    let src = f.into_raw_fd();
    let hi = move_to_high_fd(src, 10, true).unwrap();
    assert!(hi >= 10);
    // src is now closed.
    assert_eq!(unsafe { libc::fcntl(src, libc::F_GETFD) }, -1);
    let flags = unsafe { libc::fcntl(hi, libc::F_GETFD) };
    assert_eq!(flags & libc::FD_CLOEXEC, libc::FD_CLOEXEC);
    unsafe {
        libc::close(hi);
    }
}

#[test]
fn move_to_high_fd_err_on_bad_src_leaves_state_sane() {
    // A definitely-closed fd -> EBADF; the fn returns Err without panicking.
    let f = std::fs::File::open("/dev/null").unwrap();
    let bad = f.into_raw_fd();
    unsafe {
        libc::close(bad);
    }
    assert!(move_to_high_fd(bad, 10, false).is_err());
}
```

Ensure `tests.rs` has `use super::*;` and `use std::os::fd::IntoRawFd;` (add the
`IntoRawFd` import if absent). Run:
`cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (the three new
tests plus the existing suite; green).

### Step 1.2 — Rewire `relocate_high_cloexec` to the adapter; keep its callers

Locate `relocate_high_cloexec` in `executor.rs` (grep the name). **Before:**

```rust
fn relocate_high_cloexec(fd: RawFd) -> RawFd {
    unsafe {
        let new = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10);
        if new < 0 {
            // Could not relocate (e.g. EMFILE) — fall back to the original fd
            // with CLOEXEC set; collisions are unlikely in the common case.
            set_cloexec(fd);
            return fd;
        }
        libc::close(fd);
        new
    }
}
```

**After** (delegates to the shared move; keeps the best-effort-on-EMFILE policy
in one named place; note `set_cloexec` now lives in `child_fd`):

```rust
fn relocate_high_cloexec(fd: RawFd) -> RawFd {
    crate::child_fd::move_to_high_fd(fd, 10, true).unwrap_or_else(|_| {
        // Could not relocate (e.g. EMFILE) — fall back to the original fd with
        // CLOEXEC set; collisions are unlikely in the common case.
        crate::child_fd::set_cloexec(fd);
        fd
    })
}
```

Its callers (`build_child_redir_plan` numeric File, its heredoc/herestring rfd
arms, `build_child_extra_ops` File) are unchanged — they still call
`relocate_high_cloexec`. (T2 folds the File uses into `open_redirect_file`; the
heredoc-rfd uses remain callers.)

### Step 1.3 — Delete the old `set_cloexec` in executor.rs; rewire its other callers

Locate `fn set_cloexec` in `executor.rs` (grep). Delete the whole function.
Every in-executor `set_cloexec(x)` call must become
`crate::child_fd::set_cloexec(x)`. Grep `set_cloexec(` in `executor.rs`; the
non-definition call sites are in `run_coproc` (two, handled in Step 1.5) and the
`relocate_high_cloexec` adapter (Step 1.2 already qualified them). Fix any
remaining bare calls.

### Step 1.4 — Convert the `alloc_high_fd` `{var}` callers to `dup_to_high_fd`

There are two `{var}` sites. **Site A — `RedirectScope::apply_var`** (grep
`let high = match alloc_high_fd(src)`; there are two matches — this is the one
inside `apply_var`, preceded by the comment "Allocate a free high fd duped from
`src`"). **Before:**

```rust
        let high = match alloc_high_fd(src) {
            Ok(h) => h,
            Err(e) => {
                if owns_src {
                    unsafe { libc::close(src) };
                }
```

**After** (only the call changes; the error arm is identical):

```rust
        let high = match crate::child_fd::dup_to_high_fd(src, 10, false) {
            Ok(h) => h,
            Err(e) => {
                if owns_src {
                    unsafe { libc::close(src) };
                }
```

**Site B — `build_child_redir_plan` `{var}` arm** (grep the OTHER
`let high = match alloc_high_fd(src)`, the one inside `build_child_redir_plan`,
whose error arm formats `"{name}: {}"`). Same one-line change:

```rust
            let high = match crate::child_fd::dup_to_high_fd(src, 10, false) {
```

Both keep `owns_src`-driven source closing exactly as today (dup, not move).

### Step 1.5 — Convert the coproc `alloc_high_fd` callers to `move_to_high_fd`

Locate `run_coproc` (grep). Two sites, `read_fd` and `write_fd`. **Before**
(read_fd; grep `let read_fd = match alloc_high_fd(out_r)`):

```rust
    let read_fd = match alloc_high_fd(out_r) {
        Ok(hi) => {
            unsafe {
                libc::close(out_r);
            }
            set_cloexec(hi);
            hi
        }
        Err(e) => {
            unsafe {
                libc::close(out_r);
                libc::close(in_w);
            }
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
            return ExecOutcome::Continue(1);
        }
    };
```

**After** (the move absorbs the `close(out_r)` + `set_cloexec`; the Err arm
still closes `in_w`, and `out_r` too — on Err `move_to_high_fd` leaves its src
open, so closing `out_r` here stays correct):

```rust
    let read_fd = match crate::child_fd::move_to_high_fd(out_r, 10, true) {
        Ok(hi) => hi,
        Err(e) => {
            unsafe {
                libc::close(out_r);
                libc::close(in_w);
            }
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
            return ExecOutcome::Continue(1);
        }
    };
```

**Before** (write_fd; grep `let write_fd = match alloc_high_fd(in_w)`):

```rust
    let write_fd = match alloc_high_fd(in_w) {
        Ok(hi) => {
            unsafe {
                libc::close(in_w);
            }
            set_cloexec(hi);
            hi
        }
        Err(e) => {
            unsafe {
                libc::close(read_fd);
                libc::close(in_w);
            }
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
            return ExecOutcome::Continue(1);
        }
    };
```

**After:**

```rust
    let write_fd = match crate::child_fd::move_to_high_fd(in_w, 10, true) {
        Ok(hi) => hi,
        Err(e) => {
            unsafe {
                libc::close(read_fd);
                libc::close(in_w);
            }
            {
                let mut err = err_writer(err_sink, sink);
                crate::sh_error_to!(
                    shell,
                    &mut *err,
                    None,
                    "coproc: {}",
                    crate::bash_io_error(&e)
                );
            }
            return ExecOutcome::Continue(1);
        }
    };
```

### Step 1.6 — Delete `alloc_high_fd`; rewire `make_pipe`; delete `move_fd_above_stdio`

`alloc_high_fd` now has no callers — grep `alloc_high_fd`, confirm only the
definition remains, delete the whole function.

Locate `fn make_pipe` in `executor.rs` (grep; the production one returning
`io::Result<(RawFd, RawFd)>`, NOT the `#[cfg(test)]` one in `wait_loop.rs`).
**Before** (its two `move_fd_above_stdio` calls):

```rust
    let (r0, w0) = (fds[0], fds[1]);
    let r = match move_fd_above_stdio(r0) {
        Ok(fd) => fd,
        Err(e) => {
            unsafe {
                libc::close(r0);
                libc::close(w0);
            }
            return Err(e);
        }
    };
    let w = match move_fd_above_stdio(w0) {
        Ok(fd) => fd,
        Err(e) => {
            unsafe {
                libc::close(r);
                libc::close(w0);
            }
            return Err(e);
        }
    };
    Ok((r, w))
```

**After** (inline the `fd > 2` no-op conditional that `move_fd_above_stdio`
carried, using the shared move for the ≤2 case):

```rust
    let (r0, w0) = (fds[0], fds[1]);
    let r = if r0 > 2 {
        r0
    } else {
        match crate::child_fd::move_to_high_fd(r0, 3, false) {
            Ok(fd) => fd,
            Err(e) => {
                unsafe {
                    libc::close(r0);
                    libc::close(w0);
                }
                return Err(e);
            }
        }
    };
    let w = if w0 > 2 {
        w0
    } else {
        match crate::child_fd::move_to_high_fd(w0, 3, false) {
            Ok(fd) => fd,
            Err(e) => {
                unsafe {
                    libc::close(r);
                    libc::close(w0);
                }
                return Err(e);
            }
        }
    };
    Ok((r, w))
```

Now `move_fd_above_stdio` has no callers — grep it, delete the whole function
(and its doc comment). Leave `make_pipe`'s `libc::pipe`-and-error-check prologue
untouched (T3 rewrites it).

### Step 1.7 — Build, test, commit

```
cargo fmt --all
cargo build -p huck 2>&1 | tail        # warning-clean
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```

Spot-check nothing regressed at a couple of the touched paths:

```
./target/debug/huck -c 'exec 3>f; echo x >&3; exec 3>&-; cat f'   # -> x  ({var}-adjacent numeric path)
./target/debug/huck -c 'coproc C { cat; }; echo hi >&${C[1]}; read -u ${C[0]} v; echo $v'  # -> hi
```

Commit:

```
git commit -am "v291 T1: fold high-fd helpers into dup_to_high_fd/move_to_high_fd (#135)

Add dup_to_high_fd/move_to_high_fd(src,min,cloexec) + set_cloexec to child_fd.rs;
rewire alloc_high_fd callers ({var} -> dup, coproc -> move), make relocate_high_cloexec
a thin adapter, inline make_pipe's >2 no-op and delete move_fd_above_stdio/alloc_high_fd.
Behavior-preserving.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Deliverable:** engine lib tests green, warning-clean, no behavior change.

---

## Task 2 — `open_redirect_file` (the #135 fix)

Spec §T2. Add the relocating open helper, route all 11 open regions through it,
and — the actual #135 fix — give the in-process install a generalized
`new_fd == target` CLOEXEC-clear arm plus relocate the heredoc read ends.

### Step 2.1 — Add `open_redirect_file` to `executor.rs`

Locate `fn open_writable` in `executor.rs` (the noclobber leaf) — add
`open_redirect_file` just above it. It needs `FileMode` (already imported via
`huck_syntax::command`), `File`/`OpenOptions` (imported at top), and
`relocate_high_cloexec` (Step 1.2 adapter):

```rust
/// THE redirect file-open matrix: open `path` per `mode` (ReadOnly / Truncate
/// honoring `noclobber` / Clobber / Append / ReadWrite-no-truncate), then
/// relocate the fd >= 10 with FD_CLOEXEC (best-effort on EMFILE, via
/// relocate_high_cloexec) so a parent-opened redirect *source* can never land
/// in the 0..9 range that redirect *targets* operate on (#135, #132-class).
/// Callers report failures via `redir_open_error(path, ..)` as today.
fn open_redirect_file(mode: &FileMode, path: &str, noclobber: bool) -> io::Result<OwnedFd> {
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
    let file: File = match mode {
        FileMode::ReadOnly => File::open(path)?,
        FileMode::Truncate => open_writable(path, noclobber)?,
        FileMode::Clobber => open_writable(path, false)?,
        FileMode::Append => OpenOptions::new().create(true).append(true).open(path)?,
        FileMode::ReadWrite => OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?,
    };
    let raw = relocate_high_cloexec(file.into_raw_fd());
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}
```

If `OwnedFd`/`FromRawFd`/`IntoRawFd` are already imported at the executor top
level, drop the inner `use`. (This helper does NOT emit diagnostics — callers
keep their `redir_open_error(path, &e)` on the `Err` arm, so per-site error
formatting is unchanged, including the old `resolved_path(&resolved)` which was
always the input `path`.)

Note: this compiles but is unused until Step 2.2 — that is fine within the task;
do not commit between 2.1 and the conversions (avoid a dead-code warning window).

### Step 2.2 — Convert M4 + M5 (child-plan File arms) — the mechanical baseline

Start with the child-plan numeric arms (they already relocate, so this is the
purest "collapse the matrix" edit and proves the helper before touching the
in-process #135 sites).

**M4 — `build_child_redir_plan` numeric File arm.** Grep the `let file: File =
match mode {` inside `build_child_redir_plan` (the one followed by
`let raw = relocate_high_cloexec(file.into_raw_fd());`). **Before** (the whole
match through the relocate + held push):

```rust
                let file: File = match mode {
                    FileMode::ReadOnly => match File::open(&path) {
                        Ok(f) => f,
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
                            return Err(1);
                        }
                    },
                    FileMode::Truncate | FileMode::Append | FileMode::Clobber => {
                        let resolved = match mode {
                            FileMode::Append => ResolvedRedirect::Append(path),
                            FileMode::Clobber => ResolvedRedirect::Truncate(path),
                            _ if shell.shell_options.noclobber => {
                                ResolvedRedirect::NoclobberTruncate(path)
                            }
                            _ => ResolvedRedirect::Truncate(path),
                        };
                        match open_resolved(&resolved) {
                            Ok(f) => f,
                            Err(e) => {
                                redir_open_error(
                                    shell,
                                    err_sink,
                                    sink,
                                    &resolved_path(&resolved),
                                    &e,
                                );
                                return Err(1);
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        match OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(false)
                            .open(&path)
                        {
                            Ok(f) => f,
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                return Err(1);
                            }
                        }
                    }
                };
                // Relocate above fd 9 so the source never collides with a low
                // explicit-redirect target (e.g. `2>file 3>&2`).
                let raw = relocate_high_cloexec(file.into_raw_fd());
                let owned = unsafe { OwnedFd::from_raw_fd(raw) };
                plan.ops.push(ChildRedirOp::Dup {
                    target,
                    source: raw,
                });
                plan.held.push(owned);
```

**After** (the helper subsumes the matrix + the relocate; `redir_open_error`
stays on the caller's `Err`):

```rust
                let owned = match open_redirect_file(mode, &path, shell.shell_options.noclobber) {
                    Ok(fd) => fd,
                    Err(e) => {
                        redir_open_error(shell, err_sink, sink, &path, &e);
                        return Err(1);
                    }
                };
                use std::os::fd::AsRawFd;
                let raw = owned.as_raw_fd();
                plan.ops.push(ChildRedirOp::Dup {
                    target,
                    source: raw,
                });
                plan.held.push(owned);
```

**M5 — `build_child_extra_ops` File arm.** Grep the identical `let file: File =
match mode {` inside `build_child_extra_ops` (followed by `let raw =
relocate_high_cloexec(...)` and `held.push(...)`). Apply the SAME before→after
(it uses local `ops`/`held` `Vec`s, not `plan.ops`/`plan.held` — adjust the two
push targets to `ops.push(...)` / `held.push(owned)`, matching the surrounding
code).

Build + engine lib tests green before continuing.

### Step 2.3 — Convert M3 + M2 (`{var}` File arms)

**M3 — `build_child_redir_plan` `{var}` File arm.** Grep inside the `{var}`
handling (the `(fd, true)` tuple result, preceded by a `let fd: RawFd = match
mode {`). **Before:**

```rust
                let fd: RawFd = match mode {
                    FileMode::ReadOnly => match File::open(&path) {
                        Ok(f) => f.into_raw_fd(),
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
                            return Err(1);
                        }
                    },
                    FileMode::Truncate | FileMode::Append | FileMode::Clobber => {
                        let resolved = match mode {
                            FileMode::Append => ResolvedRedirect::Append(path),
                            FileMode::Clobber => ResolvedRedirect::Truncate(path),
                            _ if shell.shell_options.noclobber => {
                                ResolvedRedirect::NoclobberTruncate(path)
                            }
                            _ => ResolvedRedirect::Truncate(path),
                        };
                        match open_resolved(&resolved) {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                redir_open_error(
                                    shell,
                                    err_sink,
                                    sink,
                                    &resolved_path(&resolved),
                                    &e,
                                );
                                return Err(1);
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        match OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(false)
                            .open(&path)
                        {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                return Err(1);
                            }
                        }
                    }
                };
                (fd, true)
```

**After** (this arm's `src` is later duped high non-CLOEXEC via
`dup_to_high_fd(src, 10, false)` from Step 1.4 and then closed by `owns_src`;
so here just yield a raw fd. Since `open_redirect_file` already relocated it
high+CLOEXEC, the subsequent `dup_to_high_fd(.., false)` produces the required
non-CLOEXEC inherited copy and `owns_src` closes this CLOEXEC original —
net-identical child state, extra hygiene on the temp):

```rust
                let fd: RawFd = match open_redirect_file(mode, &path, shell.shell_options.noclobber) {
                    Ok(owned) => owned.into_raw_fd(),
                    Err(e) => {
                        redir_open_error(shell, err_sink, sink, &path, &e);
                        return Err(1);
                    }
                };
                (fd, true)
```

**M2 — `RedirectScope::apply_var` File arm.** Grep the same
`let fd: RawFd = match mode {` inside `apply_var` (yields `(fd, true)`). Apply
the IDENTICAL before→after as M3.

Build + engine lib tests green.

### Step 2.4 — Convert M1 (`RedirectScope::apply`) — the #135 fix

This is the in-process File arm plus the `new_fd == target` install. Grep
`RedirOp::File { mode, target: word } =>` inside `fn apply` (the first one;
it has the `if new_fd == target {` install block after the match). **Before**
(the open match, ending at `let new_fd: RawFd = match mode { ... };`):

```rust
                let new_fd: RawFd = match mode {
                    FileMode::ReadOnly => match File::open(&path) {
                        Ok(f) => f.into_raw_fd(),
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
                            return Err(ExecOutcome::Continue(1));
                        }
                    },
                    FileMode::Truncate | FileMode::Append | FileMode::Clobber => {
                        let resolved = match mode {
                            FileMode::Append => ResolvedRedirect::Append(path),
                            FileMode::Clobber => ResolvedRedirect::Truncate(path),
                            _ if shell.shell_options.noclobber => {
                                ResolvedRedirect::NoclobberTruncate(path)
                            }
                            _ => ResolvedRedirect::Truncate(path),
                        };
                        match open_resolved(&resolved) {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                redir_open_error(
                                    shell,
                                    err_sink,
                                    sink,
                                    &resolved_path(&resolved),
                                    &e,
                                );
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                    FileMode::ReadWrite => {
                        // `<>`: O_RDWR|O_CREAT — open in place, do NOT truncate
                        // (bash keeps existing content for read-write access).
                        match OpenOptions::new()
                            .read(true)
                            .write(true)
                            .create(true)
                            .truncate(false)
                            .open(&path)
                        {
                            Ok(f) => f.into_raw_fd(),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
                                return Err(ExecOutcome::Continue(1));
                            }
                        }
                    }
                };
```

**After** (the file is now relocated ≥10 + CLOEXEC, so with fd 0/1/2 freed the
open no longer lands on the target — this is the core #135 fix):

```rust
                let new_fd: RawFd = match open_redirect_file(
                    mode,
                    &path,
                    shell.shell_options.noclobber,
                ) {
                    Ok(owned) => owned.into_raw_fd(),
                    Err(e) => {
                        redir_open_error(shell, err_sink, sink, &path, &e);
                        return Err(ExecOutcome::Continue(1));
                    }
                };
```

Now the install block just below. **Before:**

```rust
                if new_fd == target {
                    // The kernel placed the opened file directly at the target fd,
                    // which means target was previously free/closed (lowest-free
                    // fd == target). Leave the file in place and record a
                    // "was-closed" restore (-1) so Drop closes target back when
                    // the scope ends. Do NOT dup2 and do NOT close new_fd (it IS
                    // the target now).
                    self.saved.push((target, -1));
                } else {
                    // Normal case: save the prior target state (or -1 if it was
                    // closed), dup2 the opened file onto the target, then close
                    // the temp fd. `redirect()` already records saved=-1 when
                    // dup(target) returns EBADF (target was free but not lowest).
                    if self
                        .redirect(shell, new_fd, target, sink, err_sink)
                        .is_err()
                    {
                        unsafe { libc::close(new_fd) };
                        return Err(ExecOutcome::Continue(1));
                    }
                    unsafe { libc::close(new_fd) };
                }
                Ok(())
```

**After** (the `new_fd == target` case is now reachable only for targets ≥ 10;
generalize it to ALSO clear FD_CLOEXEC in place — mirroring
`replay_redir_ops`' `source == target` arm — since dup2(fd,fd) does not clear
CLOEXEC and the relocated file is CLOEXEC'd, so it would otherwise vanish on a
later exec):

```rust
                if new_fd == target {
                    // The relocated file landed directly on `target` (only
                    // possible for target >= 10 with 10..target busy). Leave it
                    // in place and record a "was-closed" restore (-1). It is
                    // CLOEXEC'd (from open_redirect_file), and a no-op dup2 would
                    // NOT clear that — so clear FD_CLOEXEC in place, exactly as
                    // replay_redir_ops' source==target arm does, or it vanishes
                    // on a later exec (this is the #135 mechanism generalized).
                    unsafe {
                        let flags = libc::fcntl(target, libc::F_GETFD);
                        if flags >= 0 {
                            let _ = libc::fcntl(target, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                        }
                    }
                    self.saved.push((target, -1));
                } else {
                    // Normal case: save the prior target state (or -1 if it was
                    // closed), dup2 the opened file onto the target (which clears
                    // FD_CLOEXEC on target), then close the temp fd.
                    if self
                        .redirect(shell, new_fd, target, sink, err_sink)
                        .is_err()
                    {
                        unsafe { libc::close(new_fd) };
                        return Err(ExecOutcome::Continue(1));
                    }
                    unsafe { libc::close(new_fd) };
                }
                Ok(())
```

### Step 2.5 — Relocate the in-process heredoc/here-string read ends (the heredoc #135 flavor)

Two arms in `RedirectScope::apply` (Heredoc, HereString) install the
`spawn_heredoc_writer` read end directly. Grep `RedirOp::Heredoc { body, .. } =>`
inside `apply` (the arm with `self.heredoc_writers.push(pid);` then
`if self.redirect(shell, rfd, target, ...)`). **Before:**

```rust
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        self.heredoc_writers.push(pid);
                        if self.redirect(shell, rfd, target, sink, err_sink).is_err() {
                            unsafe { libc::close(rfd) };
                            return Err(ExecOutcome::Continue(1));
                        }
                        unsafe { libc::close(rfd) };
                        Ok(())
                    }
```

**After** (relocate the rfd ≥10 + CLOEXEC before install, so with the target fd
freed the read end never lands on the target — the same class as the File fix;
verified fixes `exec 3<&-; { cat <&3; } 3<<EOF`):

```rust
                match spawn_heredoc_writer(&bytes) {
                    Ok((rfd, pid)) => {
                        self.heredoc_writers.push(pid);
                        let rfd = relocate_high_cloexec(rfd);
                        if self.redirect(shell, rfd, target, sink, err_sink).is_err() {
                            unsafe { libc::close(rfd) };
                            return Err(ExecOutcome::Continue(1));
                        }
                        unsafe { libc::close(rfd) };
                        Ok(())
                    }
```

Apply the SAME `let rfd = relocate_high_cloexec(rfd);` insertion to the
`RedirOp::HereString(w) =>` arm in `apply` (grep it — identical shape).

(The `apply_var` heredoc/herestring arms yield `(rfd, true)` and then go through
`dup_to_high_fd(.., false)` + `owns_src` close — already relocated as of Step
1.4/2.3, so they need NO change here. Confirm by inspection.)

### Step 2.6 — Convert the six slot-path opens (S1–S6)

The pipeline functions open slot files and wrap them `ChildFd::from(File)`.
Convert each to `open_redirect_file` → `ChildFd::from(OwnedFd)`. There are six
regions across `run_background_sequence` (S1 stdin Read, S2 stdout, S3 stderr)
and `run_multi_stage` (S4 stdin Read, S5 stdout, S6 stderr). Each stdout/stderr
region has a Truncate/Clobber sub-arm (via `open_writable`) and an Append
sub-arm (via `OpenOptions`).

**S1/S4 — slot stdin Read.** Grep `Some(RedirectSlot::Read(word)) =>` (two
occurrences). Inside, **before:**

```rust
                    match File::open(&path) {
                        Ok(f) => ChildFd::from(f),
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
```

**After:**

```rust
                    match open_redirect_file(&FileMode::ReadOnly, &path, false) {
                        Ok(f) => ChildFd::from(f),
                        Err(e) => {
                            redir_open_error(shell, err_sink, sink, &path, &e);
```

(the trailing bail block — `restore_inline_assignments` + `bail_teardown_*` — is
unchanged; only the `match` head changes.)

**S2/S3/S5/S6 — slot stdout/stderr Truncate|Clobber.** Grep `match
open_writable(&path, guard) {` (four occurrences). **Before:**

```rust
                        match open_writable(&path, guard) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
```

**After** (reuse the existing `guard` local — it already encodes `noclobber &&
!Clobber`; feed it as the helper's `noclobber` with `FileMode::Truncate`, which
guards only when true — behavior-identical, and Clobber's `guard==false` maps to
Clobber's `open_writable(path,false)`):

```rust
                        match open_redirect_file(&FileMode::Truncate, &path, guard) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
```

**S2/S3/S5/S6 — slot stdout/stderr Append.** Grep `match
OpenOptions::new().create(true).append(true).open(&path) {` (four occurrences,
all in the slot functions). **Before:**

```rust
                        match OpenOptions::new().create(true).append(true).open(&path) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
```

**After:**

```rust
                        match open_redirect_file(&FileMode::Append, &path, false) {
                            Ok(f) => Some(ChildFd::from(f)),
                            Err(e) => {
                                redir_open_error(shell, err_sink, sink, &path, &e);
```

Ensure `FileMode` is in scope in these functions (grep — `RedirectSlot` is
already used; add `use huck_syntax::command::FileMode;` locally or qualify if
the compiler complains).

### Step 2.7 — Delete the now-dead `ResolvedRedirect` scaffolding

After M1–M5 are converted, grep `open_resolved`, `ResolvedRedirect`,
`resolved_path`. Their only callers were the matrix copies. Delete
`fn open_resolved`, `enum ResolvedRedirect`, and `fn resolved_path` if grep
confirms zero remaining references (some may remain if a copy was missed — that
is the check that every site was converted). `open_writable` stays (used by
`open_redirect_file`). Fix any unused-import warnings.

### Step 2.8 — Build, verify the four #135 flavors, commit

```
cargo fmt --all
cargo build -p huck 2>&1 | tail       # warning-clean
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```

**#135 verification** — run each and confirm byte-identical to bash (set up
`printf 'FA\n' > inA` in a scratch dir first; `H=./target/debug/huck`):

```
# 1. brace group on freed fd0
diff <(bash -c 'exec <&-; { /bin/cat; } < inA; echo end' 2>&1) \
     <($H  -c 'exec <&-; { /bin/cat; } < inA; echo end' 2>&1)   # empty diff -> FA/end
# 2. subshell on freed fd0
diff <(bash -c 'exec <&-; ( /bin/cat ) < inA; echo end' 2>&1) \
     <($H  -c 'exec <&-; ( /bin/cat ) < inA; echo end' 2>&1)
# 3. freed fd1 -> file (proof routed through the FILE, not a trailing builtin
#    write to closed fd1 which is #137, out of scope)
rm -f out; bash -c 'exec >&-; { /bin/echo hi; } > out'; b=$(cat out)
rm -f out; $H  -c 'exec >&-; { /bin/echo hi; } > out'; h=$(cat out)
[ "$b" = "$h" ] && [ "$h" = "hi" ] && echo "fd1 OK"
# 4. heredoc read end on freed fd3
diff <(bash -c 'exec 3<&-; { /bin/cat <&3; } 3<<EOF
hh
EOF
echo end' 2>&1) \
     <($H  -c 'exec 3<&-; { /bin/cat <&3; } 3<<EOF
hh
EOF
echo end' 2>&1)
```

All four must show no divergence. Commit:

```
git commit -am "v291 T2: route redirect opens through relocating open_redirect_file — fixes #135

Add open_redirect_file(mode,path,noclobber)->OwnedFd (relocated >=10 + CLOEXEC);
convert all 11 open regions (in-process apply/apply_var, child-plan, pipeline slots).
The in-process install gains a generalized new_fd==target CLOEXEC-clear arm and the
heredoc/here-string read ends relocate too, so an opened source on a freed 0/1/2 no
longer vanishes on the child's exec. Delete the dead ResolvedRedirect scaffolding.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Deliverable:** engine lib tests green; the four #135 flavors byte-match bash.

---

## Task 3 — Unify `make_pipe(cloexec)` + route the pipe sites

Spec §T3. One pipe helper in `child_fd.rs`; delete the executor + stdin_pipe
private impls; route all production sites; flip the `fd_torture` #135 cases
green; update docs.

### Step 3.1 — Add `make_pipe(cloexec)` to `child_fd.rs` (TDD)

Append to `crates/huck-engine/src/child_fd.rs` (module-level, near
`move_to_high_fd`):

```rust
/// THE pipe-creation helper. Both ends are guaranteed >= 3 so a freed std fd
/// (e.g. after `exec <&-`) can never be silently reused as a pipe end and
/// aliased onto a child's stdio (#130, and the procsub/heredoc/stdin_pipe
/// latents). `cloexec` chooses the ends' close-on-exec state: false = inherited
/// across exec (pipeline wiring, procsub /dev/fd/N, heredoc feed); true =
/// shell/embedder-internal (stdin_pipe). The relocation uses the MATCHING dup
/// flavor so a CLOEXEC end keeps CLOEXEC when moved off a low number.
pub(crate) fn make_pipe(cloexec: bool) -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0 as RawFd; 2];
    // `pipe2(O_CLOEXEC)` is Linux-only; elsewhere create then fcntl both ends.
    #[cfg(target_os = "linux")]
    let ret = unsafe {
        if cloexec {
            libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC)
        } else {
            libc::pipe(fds.as_mut_ptr())
        }
    };
    #[cfg(not(target_os = "linux"))]
    let ret = unsafe {
        let r = libc::pipe(fds.as_mut_ptr());
        if r == 0 && cloexec {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }
        r
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    let (r0, w0) = (fds[0], fds[1]);
    let r = if r0 > 2 {
        r0
    } else {
        match move_to_high_fd(r0, 3, cloexec) {
            Ok(fd) => fd,
            Err(e) => {
                unsafe {
                    libc::close(r0);
                    libc::close(w0);
                }
                return Err(e);
            }
        }
    };
    let w = if w0 > 2 {
        w0
    } else {
        match move_to_high_fd(w0, 3, cloexec) {
            Ok(fd) => fd,
            Err(e) => {
                unsafe {
                    libc::close(r);
                    libc::close(w0);
                }
                return Err(e);
            }
        }
    };
    Ok((r, w))
}
```

Unit tests — append to `child_fd/tests.rs`:

```rust
#[test]
fn make_pipe_non_cloexec_ends_are_high_and_roundtrip() {
    let (r, w) = make_pipe(false).unwrap();
    assert!(r >= 3 && w >= 3);
    assert_eq!(unsafe { libc::fcntl(r, libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
    assert_eq!(unsafe { libc::fcntl(w, libc::F_GETFD) } & libc::FD_CLOEXEC, 0);
    let msg = b"hi\n";
    assert_eq!(unsafe { libc::write(w, msg.as_ptr().cast(), msg.len()) }, 3);
    let mut buf = [0u8; 8];
    assert_eq!(unsafe { libc::read(r, buf.as_mut_ptr().cast(), buf.len()) }, 3);
    assert_eq!(&buf[..3], msg);
    unsafe {
        libc::close(r);
        libc::close(w);
    }
}

#[test]
fn make_pipe_cloexec_sets_flag_on_both_ends() {
    let (r, w) = make_pipe(true).unwrap();
    assert!(r >= 3 && w >= 3);
    assert_eq!(
        unsafe { libc::fcntl(r, libc::F_GETFD) } & libc::FD_CLOEXEC,
        libc::FD_CLOEXEC
    );
    assert_eq!(
        unsafe { libc::fcntl(w, libc::F_GETFD) } & libc::FD_CLOEXEC,
        libc::FD_CLOEXEC
    );
    unsafe {
        libc::close(r);
        libc::close(w);
    }
}
```

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (green).
`make_pipe(cloexec)` is unused so far — the executor still has its own; the next
step deletes that. To avoid a dead-code window, do 3.1 + 3.2 before building for
warnings.

### Step 3.2 — Delete the executor `make_pipe`; route its 12 callers

Locate `fn make_pipe` in `executor.rs` (the production one, now T1-rewired with
the inline `> 2` checks). Delete the whole function.

Grep `make_pipe()` in `executor.rs` — 12 call sites (subshell capture ×2,
bg/fg pipeline, coproc ×2, `make_orphan_pipe_for_eof_reader`, etc.). Replace
each `make_pipe()` with `crate::child_fd::make_pipe(false)`. (All are pipeline /
capture / coproc wiring that must be inherited across exec — non-CLOEXEC.)

**`make_orphan_pipe_for_eof_reader`** currently carries `#[allow(dead_code)]`
but IS called from the pipeline stdout arms (grep
`make_orphan_pipe_for_eof_reader()` — it has live callers). Its body
`let (r, w) = make_pipe()?;` becomes `let (r, w) = crate::child_fd::make_pipe(false)?;`.
Check whether the `#[allow(dead_code)]` is still needed: build and, if rustc
does NOT warn, delete the `#[allow(dead_code)]` line; if it warns, keep it. (Per
spec: leave its dead-code status to Phase 3 unless the allow can simply be
dropped.)

### Step 3.3 — Route `spawn_heredoc_writer` (raw pipe → non-CLOEXEC)

Locate `fn spawn_heredoc_writer` (grep). **Before:**

```rust
    let mut fds: [libc::c_int; 2] = [-1, -1];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (r, w) = (fds[0], fds[1]);
```

**After:**

```rust
    let (r, w) = crate::child_fd::make_pipe(false)?;
```

(The rest of the function — the fork, the write loop, the `close(w)` in parent —
is unchanged. The ends are now ≥3; the read end is dup2'd by consumers as
before.)

### Step 3.4 — Route `procsub.rs` (raw pipe → non-CLOEXEC)

Locate `realize_via_devfd` in `crates/huck-engine/src/procsub.rs` (grep).
**Before:**

```rust
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
```

**After** (non-CLOEXEC required: the parent-kept end must survive the consuming
command's exec for `/dev/fd/N` to resolve):

```rust
    let (read_fd, write_fd) = crate::child_fd::make_pipe(false)?;
```

(Remove the now-unused local `fds` array. If the `RawFd` import becomes unused,
drop it.)

### Step 3.5 — Route `stdin_pipe.rs` (private impl → shared, CLOEXEC)

Locate `crates/huck-engine/src/stdin_pipe.rs`. In `with_stdin_fd0`, the call
`let (r, w) = match make_pipe() {` — change to
`let (r, w) = match crate::child_fd::make_pipe(true) {` (CLOEXEC preserved).
Then **delete the private `fn make_pipe`** at the bottom of the file (grep
`fn make_pipe` in stdin_pipe.rs — the whole function + its doc comment). The
existing stdin_pipe unit tests exercise `with_stdin_fd0` and stay valid.

This closes the §H2d fd-0 hazard: with fd 0 closed at entry, the shared
`make_pipe` relocates the read end to ≥3, so `r` can never BE 0 — `dup2(r, 0)`
is real and `close(r)` no longer destroys the just-installed fd 0. (The residual
`saved = dup(0)` → EBADF best-effort bail on an already-closed fd 0 remains,
embedder-only, out of scope — leave the existing bail code as-is.)

### Step 3.6 — Build, run the fd/pipeline integration binaries

```
cargo fmt --all
cargo build -p huck 2>&1 | tail       # warning-clean
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```

Then the touched integration binaries, one at a time:

```
for t in heredoc here_string heredoc_forked_writer process_sub coproc fd_dup \
         named_fd external_fd_redirects compound_redirects function_redirect \
         builtin_fd_ordering builtin_stdout_dup builtin_pipe_flush noclobber \
         io_error bg_sequence subshell subshell_pipeline pipeline_subshell \
         pipefail sigpipe exit_inherits cmdsub_subshell; do
  echo "=== $t ==="
  ( ulimit -v 6000000; cargo test -p huck --test $t --jobs 1 -- --test-threads 1 )
done
```

pty binaries: `subshell_pipeline_pty`, `procsub_stop_pty`,
`jobcontrol_pgroup_pty` (same single-threaded `ulimit -v` form).

### Step 3.7 — Flip the #135-family `fd_torture` cases green; shrink the exclusion header

Locate `tests/scripts/fd_torture_diff_check.sh`. Update the header exclusion
comment. **Before:**

```
# Deliberately excluded until their fixing phase: stage redirect source-order (#50)
# and the in-process whole-command redirect on a freed std fd (#135, Phase 3).
```

**After:**

```
# Deliberately excluded until their fixing phase: stage redirect source-order (#50).
# (#135 — in-process whole-command redirect on a freed std fd — was fixed in v291
# Phase 2; its cases are asserted below.)
```

Add the four #135 cases (append near the freed-std-fd section, using the same
`check "<label>" '<frag>'` mechanism; `$WORK/inA` already holds `FA\n`, and the
harness `cd`s into `$WORK`):

```
# --- #135: in-process whole-command redirect on a freed std fd (v291 Phase 2 fix) ---
check "135 brace group freed fd0"  'exec <&-; { /bin/cat; } < inA; echo end'
check "135 subshell freed fd0"     'exec <&-; ( /bin/cat ) < inA; echo end'
# fd1 flavor: prove via the FILE (a trailing builtin write to a closed fd1 is #137,
# a separate open bug — keep this case independent of it).
check "135 stdout to file freed fd1" 'exec >&-; { /bin/echo hi; } > f; /bin/cat f >&2'
check "135 heredoc rfd freed fd3"    'exec 3<&-; { /bin/cat <&3; } 3<<EOF
hh
EOF
echo end'
```

Run the harness alone first:
`HUCK_BIN=./target/debug/huck bash tests/scripts/fd_torture_diff_check.sh`
(all PASS, including the four new cases).

### Step 3.8 — Update `docs/architecture.md`

Locate the stdin_pipe reference (grep `stdin_pipe.rs` in `docs/architecture.md`;
around the module map / line 45). Update its parenthetical to note the shared
pipe helper, e.g. change
`crates/huck-engine/src/stdin_pipe.rs` (CLOEXEC pipe + dup2(r, 0) save/restore …`
to note it now uses `child_fd::make_pipe(true)` (≥3-relocated, CLOEXEC). Keep
the edit minimal and factual. Grep `architecture.md` for `make_pipe`,
`alloc_high_fd`, `move_fd_above_stdio`, `relocate_high_cloexec` — if any removed
name appears, update it (the spec's sweep found only the stdin_pipe row, but
re-verify on the current doc).

### Step 3.9 — Full sweep, both binaries, commit

```
cargo build -p huck
cargo build --release --locked --bin huck
cargo fmt --all --check
ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh
```

The sweep must be green (each harness on its own default binary; do NOT override
`HUCK_BIN` for the sweep). Commit:

```
git commit -am "v291 T3: unify make_pipe(cloexec) + route the raw pipe sites; flip #135 fd_torture green

One child_fd::make_pipe(cloexec) (>=3-relocated, matching F_DUPFD/_CLOEXEC flavor)
replaces the executor + stdin_pipe private impls; route the 12 executor callers,
procsub, spawn_heredoc_writer (all non-CLOEXEC) and stdin_pipe (CLOEXEC). Closes the
stdin_pipe fd-0 aliasing hazard. Add the four #135 fd_torture cases (exclusion header
-> #50 only) and note the shared helper in architecture.md.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**Deliverable:** `fd_torture` green (incl. the four #135 cases), full sweep
green, docs updated.

---

## Final self-review (before opening the PR)

Run through each and fix inline:

- **Spec coverage §T1:** `dup_to_high_fd` + `move_to_high_fd` added; `set_cloexec`
  moved; every `alloc_high_fd` caller rewired ({var} dup / coproc move);
  `relocate_high_cloexec` is the adapter; `move_fd_above_stdio` +
  `alloc_high_fd` deleted; thresholds unchanged (3, 10). Confirm by grep: zero
  remaining `alloc_high_fd` / `move_fd_above_stdio`.
- **Spec coverage §T2:** all 11 open regions (M1–M5 + S1–S6) route through
  `open_redirect_file`; the in-process `new_fd == target` arm clears CLOEXEC;
  heredoc/here-string read ends relocated; `ResolvedRedirect`/`open_resolved`/
  `resolved_path` deleted (grep → zero references); the four #135 flavors match
  bash; H7 untouched (grep the diff for `redirs_write_stdout`,
  `final_dests_for_1_2`, `StdoutSink`, `StderrSink`, `emit_exec_spawn_diag` —
  none should appear).
- **Spec coverage §T3:** one `make_pipe(cloexec)`; executor + stdin_pipe private
  impls deleted; all production sites routed with the correct CLOEXEC flag;
  `wait_loop.rs` test-only `make_pipe` UNTOUCHED (grep the diff — `wait_loop.rs`
  should not appear); `fd_torture` header → #50 only.
- **Spec coverage §Testing:** helper + `make_pipe` unit tests present; the
  #135-family cases flipped; full sweep + integration binaries run.
- **Spec §Non-goals honored:** no `lower_redirects` / `RedirectScope`
  consolidation; no saved-fd relocation (the `redirect()`/`close_target`
  `dup(target)` sites are untouched); #137 not addressed; pipe ends still return
  `RawFd` (no `OwnedFd` migration).
- **Placeholder scan:** grep the diff for `TODO`/`FIXME`/`unimplemented!`/
  `todo!`/`...` — none.
- **Helper signature/name consistency across tasks:** `dup_to_high_fd(src, min,
  cloexec)`, `move_to_high_fd(src, min, cloexec)`, `make_pipe(cloexec)`,
  `open_redirect_file(mode, path, noclobber)`, `set_cloexec(fd)` — all
  `pub(crate)` in `child_fd` except `open_redirect_file` (executor-private);
  every call site uses the `crate::child_fd::` path (or local for
  `open_redirect_file`). Grep each name to confirm no stale signature.
- **Warning-clean:** `cargo build -p huck 2>&1 | grep -i warning` → empty;
  `cargo fmt --all --check` clean.
