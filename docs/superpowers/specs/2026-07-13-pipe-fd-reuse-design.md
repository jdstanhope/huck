# v288 — Pipeline pipe fds must not reuse a freed 0/1/2 (`read | cat` hang after `exec <&-`)

**Issue:** [#130](https://github.com/jdstanhope/huck/issues/130) — `read` (or any
command) as the first stage of a pipeline hangs when fd 0 has been closed
(`exec <&-; read x | cat`), instead of erroring `Bad file descriptor` like bash.
`divergence` + `bug` + `sev:medium`.

This is the next hang in the bash test-suite `redir` category (redir5.sub's final
`read abcde 2>&1 | grep -q 'read error'` after `exec <&-`), reached after v287
(#126) cleared the `/bin/cat & wait` hang.

## Root cause (strace-confirmed)

`make_pipe()` (`crates/huck-engine/src/executor.rs`) is a bare `libc::pipe()` that
returns the two lowest-numbered free fds. Normally fds 0–2 are occupied by the
shell's std streams, so pipe ends land on fd ≥ 3. But `exec <&-` **closes fd 0**,
freeing it, so the pipeline's `pipe()` hands the pipe's read end to fd 0:

```
close(0)            ← exec <&-
pipe2([0, 3], 0)    ← read end = fd 0, write end = fd 3
```

`run_multi_stage` wires stage 0's stdin from the literal `STDIN_FILENO` (0), which
now aliases the pipe read end. Stage 0 (`cat`#1 / the `read` builtin's forked
stage) ends up with stdin = pipe-read and stdout = that same pipe's write end, so
it reads its own output and never sees EOF → hangs. bash keeps pipe fds out of the
0–2 range (its `move_to_high_fd`), so bash's fd 0 stays closed and stage 0 fails
with EBADF immediately.

The hang only manifests when the freed fd is **0** (the first stage then reads the
pipe). The fd 1 / fd 2 variants already match bash and are not regressed by the
fix.

## Design

Confine the fix to `make_pipe()`: after creating the pipe, move each end above the
stdio range (fd ≥ 3), mirroring bash's discipline. Then a freed 0/1/2 is never
reused as a pipe end, `STDIN_FILENO` (0) keeps referring to the real (closed)
shell stdin, and stage 0 inherits the closed fd → errors like bash.

### The helper

```rust
/// Move `fd` above the stdio range (>= 3) so a freed 0/1/2 (e.g. after
/// `exec <&-`) is never silently reused as a pipeline pipe end. Returns `fd`
/// unchanged when it is already >= 3 (the common case: nothing to do). Uses
/// `F_DUPFD` (NOT `F_DUPFD_CLOEXEC`) to preserve the existing non-close-on-exec
/// semantics of pipe fds, then closes the original low fd.
fn move_fd_above_stdio(fd: RawFd) -> io::Result<RawFd> {
    if fd > 2 {
        return Ok(fd);
    }
    let newfd = unsafe { libc::fcntl(fd, libc::F_DUPFD, 3) };
    if newfd < 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        libc::close(fd);
    }
    Ok(newfd)
}
```

### `make_pipe`

```rust
fn make_pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let (r0, w0) = (fds[0], fds[1]);
    // Keep both ends off 0/1/2 so a freed std fd can't be aliased into a
    // pipeline stage's std fd (issue #130). On failure, close what we hold.
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
}
```

### Why this is safe and tight

- **No-op on the hot path.** When fds 0–2 are open (the overwhelmingly common
  case), `libc::pipe()` already returns fds ≥ 3, so `move_fd_above_stdio` returns
  them unchanged — byte-for-byte identical behavior to today.
- **Behavior-preserving fd semantics.** `F_DUPFD` (not `F_DUPFD_CLOEXEC`) keeps the
  moved fd non-close-on-exec, exactly like the raw `libc::pipe()` ends the callers
  already dup2/close by hand. No caller assumes a specific pipe fd number.
- **All ~13 `make_pipe` callers** (foreground `run_multi_stage`, the background
  pipeline, and the capture paths) get the guarantee uniformly. Other raw
  `libc::pipe` sites (`procsub.rs`, `stdin_pipe.rs`, `wait_loop.rs`, the
  heredoc-writer pipe at executor.rs:4375) are **out of scope** — not implicated by
  #130 and left unchanged per the agreed tight scope.

### Out of scope

- The `read` builtin's error **wording** (`read: Bad file descriptor` vs bash's
  `read: read error: 0: Bad file descriptor`) is a separate, pre-existing cosmetic
  divergence, not the hang. Not touched here.
- The other raw `libc::pipe` call sites.

## Testing

### Diff harness `tests/scripts/pipe_closed_fd_diff_check.sh`

Byte-identical bash↔huck, using **external-command** pipelines so the compared
output is the programs' own messages (no shell-prefix / builtin-wording
divergence), plus a functional no-hang check for the `read` repro:

| fragment | expected (both shells) |
|---|---|
| `exec <&-; cat \| cat; echo end` | `cat: …: Bad file descriptor` (×2) + `end`, rc 0 |
| `exec <&-; cat \| grep x; echo "end=$?"` | cat's fd error + `end=1` |
| `exec <&-; cat < FIXTURE \| cat` (explicit redirect on stage 0 overrides) | FIXTURE contents |
| `printf 'hi\n' \| cat; echo end` (baseline, no close — regression guard) | `hi` + `end` |
| `exec 1<&-; echo hi \| cat` (closed fd 1 — already matched; guard) | fd-1 error |

Plus a **functional no-hang guard** (the #130 repro): `exec <&-; read x | cat; echo end`
run under `timeout`; assert huck TERMINATES (no timeout) and its exit status
matches bash's. The exact `read` error wording is intentionally NOT byte-compared
(out-of-scope divergence above) — the point is that it no longer hangs.

### Regression

- Full `tests/scripts/run_diff_checks.sh` sweep stays green (all existing pipeline
  harnesses: `pipe_*`, `pipeline_*`, `compound_redirects`, etc.).
- `cargo test -p huck-engine` / `-p huck-syntax` (per-crate, single-threaded).
- Re-run the bash-suite `redir` category via its real runner (cd into the tests
  dir) to confirm the redir5.sub `read … | grep` hang is cleared; report whether
  `redir` now passes or advances to a new blocker.

## Non-goals

- `read` error-message wording alignment with bash.
- Hardening the non-pipeline `libc::pipe` call sites.
