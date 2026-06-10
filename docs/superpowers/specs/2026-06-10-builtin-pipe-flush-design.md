# huck v129 — flush stdout before fork/spawn handoff (M-118 builtin pipe-output truncation) Design

**Status:** approved design, ready for implementation plan.
**Implements:** fix M-118 — a builtin running as a forked stage (pipeline stage or
subshell body) silently loses everything after its LAST newline — plus the
closely-related external-stage output-ordering bug that shares the same root
cause. One unified rule: huck must flush Rust's `io::stdout()` before handing fd 1
to another process.
**Branch (impl):** `v129-builtin-pipe-flush`.

## Background — measured root cause

Rust's `io::stdout()` is a **`LineWriter`**: it flushes on `\n` but buffers a
trailing partial line. huck's builtins write through it (`StdoutSink::Terminal`
arm, `executor.rs:2967`/`3046`: `let mut out = io::stdout(); run_builtin(…, &mut
out, …)`). When a builtin runs as a **forked** stage it goes through
`fork_and_run_in_subshell` (`executor.rs:4502`), whose child ends with
`libc::_exit(status)` (`executor.rs:4595`). `_exit` deliberately bypasses Rust's
Drop/atexit machinery (so the parent's `history.save()` etc. don't run in the
child) — but it ALSO bypasses the `LineWriter` flush, so the builtin's buffered
trailing partial line is discarded.

Confirmed behaviour (huck vs bash 5.x), all reproduced this session:

| fragment | huck (current) | bash | cause |
|---|---|---|---|
| `printf "%s" abc \| cat` | *(empty)* | `abc` | child `_exit` drops the unflushed buffer |
| `printf "x\ny\nz" \| cat` | `x\ny\n` | `x\ny\nz` | only the trailing partial line `z` is lost |
| `echo hello \| cat` | `hello\n` | `hello\n` | OK — `echo`'s own `\n` flushes the LineWriter |
| `printf "%s" abc \| head` | *(empty)* | `abc` | same (any downstream reader) |
| `( printf x )` | *(empty)* | `x` | same `_exit` path for a subshell body |
| `v=$(printf "%s" abc); echo "[$v]"` | `[abc]` | `[abc]` | OK — `$()` Capture is an in-memory buffer, no fork/LineWriter |

A second, closely-related divergence shares the SAME root cause — the parent's
`LineWriter` not being flushed before it forks/spawns a child that competes for
fd 1:

| fragment | huck (current) | bash | cause |
|---|---|---|---|
| `printf x; /usr/bin/printf y \| cat` | `yx` | `xy` | parent's buffered `x` flushes at parent exit, AFTER the spawned stage wrote `y` |
| `printf x; /usr/bin/printf y` | `yx` | `xy` | same, via `run_subprocess` (bare external) |
| `printf x; ( /usr/bin/printf y )` | `yx` | `xy` | same, external inside a subshell |

These are intertwined: the M-118 child-side flush is REQUIRED, but adding it
WITHOUT a parent-side flush would INTRODUCE a duplication bug — the child
inherits the parent's `LineWriter` buffer at `fork()`, so `printf x; printf y |
cat` would flush `x` in BOTH the child (→ pipe → `cat`) and the parent (→ exit),
printing `xyx`. So the parent-side flush is mandatory for correctness, and once
present it also fixes the external-ordering cases above for free.

## Architecture — flush `io::stdout()` at every fd-1 handoff

The fix is the standard fork+stdio discipline: **the parent flushes
`io::stdout()` before it forks or spawns any child, and each in-process forked
child flushes `io::stdout()` before `_exit`.** No buffering strategy changes; the
LineWriter stays. `io::stderr()` is UNBUFFERED in Rust, so it needs no flush
(a flush there is a harmless no-op and is omitted).

Add a tiny module-private helper for clarity and grep-ability:
```rust
/// Flush huck's buffered stdout (a LineWriter) before handing fd 1 to another
/// process — a fork child would otherwise inherit (and possibly duplicate) the
/// buffer, and a spawned peer would otherwise race ahead of pending parent bytes.
fn flush_stdout() {
    use std::io::Write;
    let _ = io::stdout().flush();
}
```

### The four handoff sites (all in `src/executor.rs`)

1. **`fork_and_run_in_subshell` — parent, before `libc::fork()`** (~line 4513).
   `flush_stdout();` immediately before the `let pid = unsafe { libc::fork() };`.
   Empties the parent buffer so the child inherits nothing to duplicate; also
   orders any pending parent bytes ahead of the child's output. Covers BOTH
   in-process pipeline stages and standalone/background subshell bodies (every
   caller of this helper).

2. **`fork_and_run_in_subshell` — child, before `libc::_exit(status)`** (line
   4595). `flush_stdout();` immediately before `unsafe { libc::_exit(status) };`.
   **The M-118 core fix** — writes the builtin's own trailing partial line to the
   dup2'd fd 1 (pipe or terminal) before the image exits. Placed AFTER the body
   has run and the status is computed, BEFORE `_exit`.

3. **`spawn_external_with_fds` — before the spawn** (~line 4652, at the top of the
   function body before `process.spawn()`). `flush_stdout();`. Fixes ordering for
   external pipeline stages (foreground `run_multi_stage` call site ~3830 and
   background `run_background_sequence` call site ~1939 both route through here).

4. **`run_subprocess` — before `process.spawn()`** (~line 3200, before the
   `match process.spawn()`). `flush_stdout();`. Fixes ordering for a bare
   (non-pipeline) external command.

### Why these four are sufficient
- In-process forked children (builtins, functions, compounds, subshell bodies)
  ALL exit through the single `_exit` at 4595 → site 2 covers their own output;
  they ALL fork through the single `libc::fork()` at 4513 → site 1 covers the
  inherited-buffer/ordering hazard.
- External children are spawned through exactly two paths —
  `spawn_external_with_fds` (all pipeline/background external stages) and
  `run_subprocess` (bare external) → sites 3 and 4. `std::process::Command`
  exec's a fresh image, so there is no inherited Rust buffer to flush in the
  external child; only the parent-ordering flush is needed.
- The Capture sink (`$()`/backticks) writes to an in-memory `Vec<u8>`, never the
  LineWriter, and `run_substitution` clones rather than forks — so it is already
  correct and untouched (verified: `v=$(printf "%s" abc)` → `[abc]`).

## Scope & must-not-regress
- **Newline-terminated output** (the overwhelmingly common case) is unchanged —
  the LineWriter already flushed at each `\n`; the new flush is then a no-op.
- **No duplication:** `printf x; printf y | cat` → `xy` (parent flushes `x`
  before forking, so the child inherits an empty buffer). Explicitly tested.
- **`echo` / terminated builtins** unchanged.
- **Capture / `$()`** unchanged (in-memory buffer).
- **Terminal interactivity:** no change to prompt/echo behaviour — the flush only
  fires at a fork/spawn boundary, which already implies the parent is about to
  block on a child.
- **Performance:** a flush at a fork/spawn boundary is negligible relative to the
  fork/exec it precedes.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | Add `flush_stdout()` helper; call it at the 4 handoff sites (parent-before-fork 4513, child-before-_exit 4595, `spawn_external_with_fds` top, `run_subprocess` before spawn). |
| `tests/scripts/builtin_pipe_flush_diff_check.sh` (NEW) | Bash-diff harness — fragments below, asserting byte-identical huck vs bash output. |
| `tests/` (integration) | A few `#[test]`s asserting the exact bytes for the core cases (so the fix is locked in even where a harness fragment is awkward). |
| `docs/bash-divergences.md` | DELETE the M-118 entry; decrement the Tier-1 count (2→1). |

## Testing

1. **Bash-diff harness** `tests/scripts/builtin_pipe_flush_diff_check.sh`
   (gold-standard; runs each fragment through bash and huck, asserts
   byte-identical). Fragments:
   - `printf "%s" abc | cat`                         (builtin trailing partial line, piped → `abc`)
   - `printf "x\ny\nz" | cat`                          (only-last-line-unterminated → `x\ny\nz`)
   - `printf "%s" abc | head`                          (different downstream reader)
   - `printf "a\nb" | tr a-z A-Z`                      (two builtins, first unterminated → `A\nB`)
   - `echo hello | cat`                                (terminated — must stay `hello\n`)
   - `printf x; printf y | cat`                        (no-duplication → `xy`)
   - `printf x; /usr/bin/printf y | cat`               (external ordering, piped → `xy`)
   - `printf x; /usr/bin/printf y`                     (external ordering, bare → `xy`)
   - `( printf x )`                                    (builtin in a subshell → `x`)
   - `printf x; ( /usr/bin/printf y )`                 (external in a subshell → `xy`)
   - `for i in 1 2 3; do printf "$i"; done | cat`      (loop-of-builtins, no trailing NL → `123`)

   Harness note: fragments are run as FILE-ARGS, not piped stdin (L-27 — huck
   history-expands piped non-interactive stdin).

2. **Integration `#[test]`s** asserting exact bytes for: `printf "%s" abc | cat`
   → `abc`; `printf "x\ny\nz" | cat` → `x\ny\nz`; `printf x; printf y | cat` →
   `xy` (no-dup); `v=$(printf "%s" abc); echo "[$v]"` → `[abc]\n` (Capture
   unaffected).

3. **Full regression:** the whole unit + integration suite and ALL existing
   bash-diff harnesses green; clippy clean.

## Edge cases & notes
- The child-side flush at site 2 must come AFTER the body runs (the builtin must
  have written its output into the LineWriter) and BEFORE `_exit`. It does not run
  any Drop/atexit logic — it is a single explicit `flush()` on fd 1, leaving the
  intentional `_exit` semantics (no `history.save()`) intact.
- `io::stderr()` is unbuffered in Rust; builtin stderr writes are not affected, so
  no stderr flush is added (keeps the change minimal).
- No `set -o`/option interaction; no new shell state; purely an I/O-flush
  discipline fix.
- After this lands, M-118 is fully resolved; no residual sub-symptom is expected
  (the external-ordering sibling is fixed in the same change, so it is NOT logged
  as a new deferred entry).
