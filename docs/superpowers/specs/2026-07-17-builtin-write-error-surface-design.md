# v308 — builtin write-error surface: replace the fd-1 sink

**Issues:** [#186](https://github.com/jdstanhope/huck/issues/186) (read-only fd
writes silently succeed), [#190](https://github.com/jdstanhope/huck/issues/190)
(newline-dependent, inconsistent wording), and
[#191](https://github.com/jdstanhope/huck/issues/191) (failed output leaks to the
restored fd 1).

**Goal:** builtin writes to a real fd report exactly what bash reports, in bash's
wording, and never deliver failed output anywhere else.

---

## Why one iteration covers three issues

All three are the same root: builtin output routed to a real fd goes through the
**process-global `io::stdout()`**, which lies in three different ways.

1. It **swallows EBADF** (`std::io::stdio::handle_ebadf` upstream): the write
   reports success though the syscall failed. Nothing downstream can detect it →
   #186.
2. It is a **`LineWriter`**, so whether a non-EBADF error surfaces at `write_all`
   or at a later `flush` depends on a **trailing newline**. Two different
   reporters, two different wordings → #190.
3. It **retains** unwritten bytes after a failed write. The redirect scope then
   restores fd 1 and a later flush emits them to the wrong destination → #191.

Fixing them separately would mean editing the same code three times, and #191 is
unreachable from the probe-based approach (see Rejected alternatives).

## Measured facts (bash 5.2.21 and Rust, on this box)

Everything below was measured, not assumed. A read-only fd is `exec 3</etc/hostname`;
`/dev/full` supplies ENOSPC, an errno Rust does *not* swallow.

**Rust's `io::stdout()`**, fd 1 replaced as noted:

| condition | `write` | `flush` |
|---|---|---|
| read-only fd, `"hello\n"` | `Ok(6)` | `Ok(())` |
| read-only fd, `"hello"` | `Ok(5)` | `Ok(())` |
| closed fd | `Ok(6)` | `Ok(())` |
| ENOSPC, `"hello\n"` | `Err(ENOSPC)` | `Ok(())` |
| ENOSPC, `"hello"` | `Ok(5)` | `Err(ENOSPC)` |
| EPIPE, `"hello\n"` | `Err(EPIPE)` | `Ok(())` |

EBADF is swallowed at both entry points regardless of newline. Other errnos
surface at exactly one of them, selected by the trailing newline.

**Raw `write(2)`** (this is why empty writes need care):

| call | result |
|---|---|
| `write(ro_fd, "", 0)` | `-1`, EBADF |
| `write(closed_fd, "", 0)` | `-1`, EBADF |
| `write(ro_fd, "x", 1)` | `-1`, EBADF |

**bash's behavior** — the target:

| case | bash |
|---|---|
| `echo x >&3` (ro) | `echo: write error: Bad file descriptor`, rc 1 |
| `pwd >&3` | `pwd: write error: Bad file descriptor`, rc 1 |
| `declare -p x >&3` | `declare: write error: Bad file descriptor`, rc 1 |
| `export -p >&3` | `export: write error: Bad file descriptor`, rc 1 |
| `echo -n '' >&3`, `printf '' >&3`, `: >&3`, `true >&3`, `jobs >&3` (no jobs), `cd /tmp >&3` | **silent, rc 0** — no bytes written, no write attempted |
| `echo x >&3; echo x >&3` | reports **twice** — per builtin invocation |
| `echo x >&3` with fd 3 O_RDWR | silent, rc 0 |
| `declare -p NOPE >&3` | only `declare: NOPE: not found`, rc 1 — nothing was written to fd 1, so no write error |
| any of the above to `/dev/full` | same wording with `No space left on device` |
| builtin writing to a **read-only fd 2** | rc 1, **no message** (its stderr is the broken fd) |
| all cases | **nothing** on the real stdout |

The zero-byte row is load-bearing: bash reports only when a `write(2)` actually
failed, and no write happens for empty output.

## Architecture

**Rule: a builtin's STDOUT bound for a real fd never passes through the
process-global `io::stdout()`.** (Builtin *stderr* is deliberately excluded — see
Scope boundaries.)

A new unbuffered `FdWriter` implements `std::io::Write` over a raw fd:

It takes the fd as a **parameter** rather than hardcoding 1. That is a
testability requirement, not generality for its own sake: a unit test that swaps
the process-global fd 1 cannot be reliable in a `#[cfg(test)]` module, because
`dup2` clears `O_CLOEXEC` and concurrently forking tests inherit it — a hazard
this repo has already been bitten by (`tests/tee_inherit.rs`, #90). With an fd
parameter the tests point at a temporary fd and the hazard never arises.

- `write` calls `libc::write(fd, …)`, retrying `EINTR` and returning the true
  errno. A short count is returned as-is; `write_all` loops.
- **Empty input short-circuits to `Ok(0)` without a syscall.** A zero-byte
  `write(2)` to a bad fd returns EBADF (measured above), so without this
  `echo -n '' >&3` would report where bash is silent. This is bash's
  "only if bytes were written" rule, and it is the same semantic v298 encoded as
  `!fd1_discard.is_empty()` — relocated into the writer where it cannot be
  forgotten.
- It **records the first errno** it sees and exposes it to the epilogue.
- `flush` is a no-op (nothing is buffered).

Every builtin already receives `out: &mut dyn Write`, so **no builtin changes**.

### The two conversion sites

Both write builtin **stdout** to real fd 1 and both currently use `io::stdout()`:

- `executor.rs:1493` — the main `write_to_fd1` branch.
- `executor.rs:1450` — the `(StdoutSink::Terminal, StderrSink::Merged)` arm of
  `route_out_to_err` (a final `>&2` where fd 2 is merged onto fd 1). This is a
  sibling of the same bug; converting only 1493 would leave it divergent.

### What gets deleted

v298 (#137) built its machinery *around* the EBADF swallowing. Once the sink no
longer swallows, the workarounds are moot and go — no backstop:

- `fd1_closed` (the `fcntl(1, F_GETFD)` probe) — a raw write returns EBADF for a
  closed fd on its own.
- `fd1_discard` (the throwaway `Vec` and its `!is_empty()` guard).
- the `stdout_flush` `Result` check at `executor.rs:1564-1567`.

### Ordering

`run_builtin_with_redirects` already flushes `io::stdout()` at `executor.rs:1331`
so prior output is not diverted into the redirect target. That flush now carries
a **second** obligation: it guarantees the global buffer is empty before any raw
write, so buffered output cannot be overtaken by a raw write to the same fd. Its
comment must state both reasons — an unstated rationale is what rots.

## Error reporting

**Exactly one place formats a write error.** If the writer recorded an errno, the
epilogue emits, reusing the existing site (`executor.rs:1576-1585`) and wording:

```
<name>: write error: <strerror>        // bash_io_error(&e); rc forced to Continue(1)
```

This is forced by the code's shape: **82 sites discard the write result**
(`let _ = writeln!(out, …)` — `declare`, `jobs`, `export`, …) against **6 that
check** it. Fixing the 6 would leave `declare -p x >&3` silently succeeding, and
editing 82 sites would neither be reviewable nor survive the next builtin added.
A recording writer covers all 88 for free: a discarded `Result` no longer means a
discarded error.

The 6 checking sites (`echo` ×2 at `builtins.rs:680`/`684`, `pwd` at `655`,
`export` at `1159`, `readonly` at `1825`, `jobs` at `4284`) **keep their early
return** — stop writing once the fd is broken — but **drop their message**.
`printf`'s write site (`builtins.rs:4144`), which reports the raw `io::Error` and
is the source of `(os error 28)`, gets the same treatment — making 8 sites in
all. **`builtins.rs:4025` is NOT one of them**: despite its identical
`"printf: {e}"` shape it reports a *format-parse* failure, not a write, and is
not part of this surface.

That makes #190 unrepresentable: one formatter, one wording, no newline-dependent
path selection.

## Scope boundaries

- **Builtin stderr is unchanged.** `err_writer`'s `StderrSink::Terminal` →
  `io::stderr()` and its `Merged` → `io::stdout()` arm (`executor.rs:117`) carry
  *diagnostics*. bash does not report a write error for a failed diagnostic
  either, and a builtin writing to a read-only fd 2 already matches bash
  (rc 1, no message — measured). No repro leaks through this path. If a
  differential test later demonstrates a leak there, file a follow-on rather than
  widen this iteration.
- **Read-side wording is out of scope.** `read` (`builtins.rs:3264`) and
  `mapfile` (`2968`/`2989`) have the same ad-hoc shape, but bash's read-side
  wording is `read error: <n>: <strerror>` — a similar-but-distinct family.
  Tracked in #190's notes; do not assume it is identical.
- **Externals are already correct** and are not touched (#186 confirms this).

## Testing

- **Unit tests** for `FdWriter` against real fds: EBADF surfaces (read-only and
  closed), ENOSPC surfaces, partial writes complete, EINTR retries, and
  **`write(b"")` performs no syscall and returns `Ok(0)`** — pinned against the
  measured `write(ro_fd, "", 0)` → EBADF, so the test fails if the
  short-circuit is dropped.
- **A `builtin_write_error_diff_check.sh` extension** (the harness exists, from
  v298) covering every row of the bash table above: the four zero-byte/no-output
  silent cases, `echo`/`printf`/`pwd`/`declare -p`/`export -p` on a read-only fd,
  the double-report case, the O_RDWR control, and `/dev/full` for ENOSPC.
  Newline and no-newline variants of `echo`/`printf` are **both** required —
  that pair is what distinguishes #190's two reporters.
- **A leak differential**: for each failing-write case, assert the real stdout is
  byte-empty (`huck -c '…' 2>/dev/null | od -c`). This is #191's only gate; the
  four leak shapes measured (`echo x`, `echo -n x`, `printf 'x'`, `declare -p x`
  → `/dev/full`) all become empty.
- **Regression**: v298's existing closed-fd cases must stay green with its
  machinery deleted — that is the proof the deletion lost nothing.

## Rejected alternatives

- **Extend the probe** (`fcntl(1, F_GETFL) & O_ACCMODE == O_RDONLY` beside the
  closed-fd probe, keeping `io::stdout()`). Cheap and fixes #186 alone, but
  **cannot** fix #191: the leak's trigger is ENOSPC, and no probe can predict a
  disk about to fill. It also leaves #190's two reporters in place.
- **An owned `BufWriter` over the raw fd.** Better syscall batching, and it makes
  the discard explicit. Rejected as unnecessary: builtin writes are per-line or
  per-invocation, and `io::stdout()` is itself line-buffered today, so the
  syscall count is comparable. The buffer is precisely what made these bugs
  subtle; reintroducing one we must remember to discard trades a structural fix
  for a disciplined one. Revisit only if measurement shows it matters.

## Risks

- **Syscall count** rises for builtins doing many small writes (`declare -p` with
  many variables writes per line). `io::stdout()` line-buffers, so per-line
  output already costs a syscall per line; the delta is confined to
  sub-line writes. Not expected to matter; the `BufWriter` variant is the
  fallback if it does.
- **Ordering** depends on the `executor.rs:1331` flush, which is why it stays and
  is documented rather than left implicit.
