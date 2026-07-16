# `exec` heredoc hang: adopt bash's pipe-or-tempfile body delivery — Design

**[#169](https://github.com/jdstanhope/huck/issues/169)** — `exec` with a >64KB
heredoc/here-string hangs (permanent redirect reaps the writer before any reader
drains). Labelled `sev:low` as an exotic invocation, but it is a genuine,
deterministic **hang**.

Scope: `crates/huck-engine/src/executor.rs` — `spawn_heredoc_writer` and every
path that threads its writer pid (`RedirectScope`, `ChildRedirPlan`, the
foreground/background pipeline spawn paths).

---

## The bug

```
$ huck -c 'exec 3<<<"$(head -c 70000 /dev/zero | tr "\0" x)"; echo rc=$?'
# hangs
$ bash -c 'exec 3<<<"$(head -c 70000 /dev/zero | tr "\0" x)"; echo rc=$?; head -c5 <&3; echo'
rc=0
xxxxx
```

`apply_redirects_permanently` (`executor.rs:4674`) installs the heredoc pipe's
read end onto the target fd, then on the success path calls
`scope.reap_heredoc_writers()` before dropping the scope. For a **permanent**
(`exec`) redirect there is no reader — the reader is a *later* command — so a
body larger than the pipe buffer leaves the forked writer blocked on a full
pipe, and the synchronous `waitpid` never returns.

The limitation is already documented in-tree at `executor.rs:4715`. Unlike the
temporary appliers (fixed for
[#142](https://github.com/jdstanhope/huck/issues/142) in v303 by restore-then-reap
in `Drop`), `exec` cannot reorder its way out: the writer legitimately must stay
alive until the fd is drained, and nothing will drain it during `exec`.

## bash 5.2.21 behavior (verified, not assumed)

bash has no writer process to reap **at any size**, which is why it cannot hit
this hang. `here_document_to_fd` picks one of two mechanisms by body length:

| body length | mechanism | verified via |
| --- | --- | --- |
| ≤ 65536 bytes | pipe, written **directly by the parent**, no fork | `readlink /proc/$$/fd/3` → `pipe:[…]` |
| > 65536 bytes | **unlinked temp file**, rewound to offset 0 | `readlink /proc/$$/fd/3` → `/tmp/sh-thd.XXXXXX (deleted)` |

Boundary probed empirically: a 65535-byte here-string body (herelen 65536
including the appended newline) yields a pipe; 65536 bytes (herelen 65537)
yields a temp file. So the rule is `herelen <= 65536 → pipe`, where `herelen`
includes the here-string's trailing newline. 65536 is the Linux default pipe
capacity — the parent's blocking write is safe only because the body always
fits.

Further verified properties of bash's temp-file path:

- **The fd is read-only.** `/proc/$$/fdinfo/3` reports `flags: 0100000`
  (`O_LARGEFILE`, access mode `O_RDONLY`), and `exec 3<<<BIG; echo x >&3` gives
  `write error: Bad file descriptor` — identical to the pipe case. bash reaches
  this by reopening the file `O_RDONLY` *before* closing the writable fd.
- **`TMPDIR` is honored, with a silent `/tmp` fallback.** `TMPDIR=/tmp/mytd`
  → `/tmp/mytd/sh-thd.XXXXXX`; both `TMPDIR=/nonexistent/xx` and
  `TMPDIR=/proc` (unwritable) fall back to `/tmp` with rc 0 and no diagnostic.
- File mode is owner-only; it is unlinked immediately, so it is unreachable by
  name regardless.

## Approach: full bash-model replacement (chosen)

Rejected alternatives:

- **Narrow, `exec`-only** — keep the forked writer everywhere, spool to a temp
  file only for permanent redirects. Contained and safe, but adds a tenth
  special case to machinery that is already duplicated per exec path, and
  leaves the fork-per-heredoc design in place.
- **Lazy reaping** — leave the writer running at `exec` time, reap at fd
  close / shell exit. Leaves a long-lived blocked child holding a copy of the
  shell's fds, and entangles with `wait`/`$!`/the job table. The issue text
  itself argues against it.

The wholesale replacement makes the hang **unreachable by construction** (no
writers exist to block on) and is mostly *subtraction* — it collapses a
mechanism currently duplicated across the fg-pipeline, bg, subshell, and capture
paths onto one helper. That is the systematic direction the recent fd/redirect
bug cluster calls for, rather than another per-path patch.

### The new helper

`spawn_heredoc_writer(bytes) -> (RawFd, pid_t)` is replaced by:

```rust
fn heredoc_body_to_fd(bytes: &[u8]) -> Result<RawFd, io::Error>
```

One fd out; no pid, no fork. Every caller loses its writer-tracking obligation.

The size rule is checked against **`bytes.len()`**, which *is* bash's `herelen`:
the here-string call sites already `bytes.push(b'\n')` before calling, so the
trailing newline is included in the measured length, matching the boundary
probed above.

**Path 1 — pipe (body ≤ 65536).** Create the pipe, set `O_NONBLOCK` on the
**write end only**, write the body, close the write end, return the read end.
`O_NONBLOCK` is a property of the open file description and the read end is a
distinct description, so the consumer's reads stay blocking — the probe is
invisible downstream. On a short write or `EAGAIN`, the platform's pipe is
smaller than Linux's: close both ends and fall through to path 2.

The size check comes first (so a large body does no wasted pipe work, matching
bash exactly on Linux); the nonblocking probe is the portability guard. bash
hardcodes 65536 and writes *blocking*, which is safe only on a 64KB-pipe
platform — on macOS, where pipes start at 16KB, that same code has nothing to
stop it wedging. Given [#97](https://github.com/jdstanhope/huck/issues/97) is
already a macOS-only hang, huck degrades to a temp file there instead of
inheriting bash's exposure.

**Path 2 — temp file (body > 65536, or the probe fell short).** Follow bash's
exact, race-conscious sequence:

1. `mkstemp` in `$TMPDIR` (fall back to `/tmp` when unset or unusable), mode
   0600 → a read-write fd
2. write the whole body
3. `open(path, O_RDONLY)` — the second fd, opened **before** the first is
   closed, which is how bash avoids a race on the name
4. `unlink(path)` — the file is now anonymous; nothing can open it by name, and
   it vanishes if we crash
5. `close` the read-write fd; return the read-only one

### Invariants

- The returned fd is **read-only** on both paths, so `exec 3<<<x; echo y >&3`
  gives `Bad file descriptor` exactly as bash does.
- The helper's contract is "a fresh readable fd positioned at offset 0" — true
  of a pipe read end and a rewound file alike, so **no call site cares which
  path produced it**. Callers keep applying `relocate_high_cloexec` as today.
- After `heredoc_body_to_fd` returns, the body is **already fully delivered**
  (pipe buffer or unlinked inode). There is no second process for `exec` — or
  anything else — to wait on.
- All body sizes still work on every path: ≤64KB fits the pipe; >64KB goes to a
  file. Nothing regresses relative to the concurrent-writer model.

## Deletion scope

The audit confirms every producer of a `heredoc_writers` pid is a
heredoc/here-string spawn — nothing else pushes to those vectors. So the whole
mechanism goes:

- `spawn_heredoc_writer` (`executor.rs:3510`) and the `writers: &mut Vec<pid_t>`
  out-param threaded through `lower_one_redirect` (`:4985`), plus its four push
  sites (`:5049`, `:5072`, `:5267`, `:5298`)
- `RedirectScope::heredoc_writers` (`:970`) and `reap_heredoc_writers` (`:1115`),
  including the reap in `Drop` (`:1152`)
- `ChildRedirPlan::heredoc_writers` (`:4903`, `:4953`) and the plan-build /
  teardown reaps (`:5374`, `:5381`)
- the foreground pipeline's local vec (`:6142`), its `Foreground`/`Background`
  match arms (`:6460`, `:6504`), the `append` (`:6631`), and the reap loops
  (`:5781`, `:5791`, `:5809`, `:7080`)

### Consequences

- **Retires the #142 restore-then-reap invariant** in `Drop`. Not a regression:
  #142's hang *was* a blocked writer, so removing writers removes the failure
  mode the ordering defended against. #142's regression test is **kept as-is**
  and must still pass — on behavior, not mechanism.
- **Removes a class of stray `SIGCHLD`.** The writers were, per the `M-120`
  comments, "not jobs, not `$!`" — invisible children every job-control path had
  to know to ignore.
- **Removes the `Foreground`/`Background` asymmetry** at `:6460`, where
  background writers were silently left to SIGCHLD.
- **Stops duplicating the body into a child address space** — relevant to the
  soak harness's counters.
- `crates/huck-engine/tests/exec_redirect_no_leak.rs`
  ([#178](https://github.com/jdstanhope/huck/issues/178)) still applies: the
  `drop(scope)` (not `mem::forget`) discipline in `apply_redirects_permanently`
  is unchanged.

## Error handling

Both paths keep the existing `heredoc: <errno>` diagnostic and `Err(1)` from
`lower_one_redirect` — no new call-site shapes, no new emitter routing.

We deliberately do **not** try to match bash's "cannot create temp file for
here-document" wording: it could not be provoked (bash silently falls back to
`/tmp` even with `TMPDIR=/proc`), and inventing a message unverifiable against
5.2.21 is how prior error-text divergences were created. Error text is a stated
non-goal.

## Testing

**Unit** (`huck-engine`): `fstat` the fd returned by `heredoc_body_to_fd` and
assert `S_ISFIFO` for a 65536-byte body, `S_ISREG` for 65537 — this pins the
threshold and path selection deterministically, which a diff harness
structurally cannot do. Plus content round-trip at both sizes, and a write to
the fd failing `EBADF`.

**Bash-diff** (`tests/scripts/heredoc_exec_diff_check.sh`, the gold standard per
CLAUDE.md): `exec 3<<<BIG` then `rc=$?`, `head -c5 <&3`, `wc -c <&3`; the same
for `<<EOF`; the write-to-fd `EBADF` case; several `exec` heredocs in one shell;
bodies straddling the boundary.

The fd *type* stays **out** of the diff harness deliberately: `readlink
/proc/$$/fd/3` yields a different temp path per process and could never be
byte-identical. That check belongs in the unit layer, via `fstat`.

**Anti-hang**: every regression test for the hang wraps huck in `timeout 10`, so
a regression **fails** the suite rather than wedging CI.

Output here is fully deterministic — with no writer process there is no
concurrent writer/consumer race — so the byte-diff gate is trustworthy, unlike
the [#151](https://github.com/jdstanhope/huck/issues/151) case where a
nondeterministic SIGPIPE race made a green diff meaningless.

**Differential gate**: run `tools/redirect_audit.sh` (the standing gate for any
fd change), and exercise **all four exec paths** — fg pipeline, bg, subshell,
capture — not just `exec`. The point of unifying them is that they now share one
helper; that claim needs checking, not assuming.

**Run discipline** (per the OOM notes): per-crate
`cargo test -p <crate> --jobs 1 -- --test-threads 1`; the `-p huck` integration
binaries run individually before pushing; `ulimit -v` guard on harness sweeps.

## Out of scope

- bash's exact temp-file *name* (`sh-thd.XXXXXX`) and mode — unobservable, the
  file is unlinked immediately.
- Error-message wording (above).
- [#97](https://github.com/jdstanhope/huck/issues/97) (macOS procsub Ctrl-Z
  hang) — unrelated mechanism; this design only avoids *adding* macOS exposure.
