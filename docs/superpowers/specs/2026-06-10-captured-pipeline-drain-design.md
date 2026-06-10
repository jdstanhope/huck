# huck v133 — drain a captured pipeline concurrently (M-119 deadlock) Design

**Status:** approved design, ready for implementation plan.
**Implements:** fix M-119 — `x="$(producer | filter)"` deadlocks when the captured
output exceeds the OS pipe buffer (~64 KB). The remaining `nvm ls-remote` hang.
**Branch (impl):** `v133-captured-pipeline-drain`.

## Background — measured root cause

`run_multi_stage` (src/executor.rs) keeps the capture pipe's read end
(`capture_read_fd`) open and drains it into the substitution buffer (`io::copy`)
only AFTER `wait_pipeline_raw` (executor.rs:4103) has reaped every stage. During
the wait nothing reads the capture pipe, so once the final stage's output exceeds
the ~64 KB pipe buffer it blocks on `write()`, never exits, `wait_pipeline_raw`
never returns, and the post-wait read at ~4117 is never reached → classic
reader/writer deadlock.

The SINGLE-command capture path already does the right thing — it drains BEFORE
waitpid (executor.rs:378, comment "Drain capture pipe before waitpid to avoid
deadlock"), which is why `x="$(seq 1 500000)"` (3.4 MB, no pipe) works but
`x="$(seq 1 500000 | cat)"` (pipeline) hangs.

| case | huck (current) | bash |
|---|---|---|
| `x="$(seq 1 500000)"` (no pipe) | ✅ 3388894 | ✅ |
| `x="$(seq 1 1000 \| cat)"` (small pipeline) | ✅ 3892 | ✅ |
| `x="$(seq 1 500000 \| cat)"` (large pipeline) | **HANG** | ✅ 3388894 |
| ordinary `seq 1 500000 \| cat` (no capture) | ✅ | ✅ |

`nvm ls-remote`'s `VERSION_LIST="$(nvm_download … | command sed …)"` pipes the
~200 KB `index.tab` through a filter inside a capture → exactly this pattern.

## Architecture — drain before wait (mirror the single-command path)

`bash` reads the capture pipe concurrently with the pipeline running. huck's stages
are separate processes, so a single sequential drain-to-EOF in the parent already
overlaps with the children writing — no thread needed (huck is single-threaded;
`Rc` not `Arc`). The fix relocates the existing capture-read so it runs before the
wait.

### The change (one site, `run_multi_stage`)
Move the capture-sink read block — currently AFTER `wait_pipeline_raw`
(~executor.rs:4117-4126):
```rust
// ---- Read capture sink ----
if let Some(r) = capture_read_fd.take() {
    if let StdoutSink::Capture(buf) = sink {
        let mut f = unsafe { File::from_raw_fd(r) };
        let _ = io::copy(&mut f, *buf);
    } else {
        unsafe { libc::close(r); }
    }
}
```
to BEFORE the wait — right after the parent-held-fd cleanup (~executor.rs:4095,
the `parent_held.retain(|&fd| Some(fd) == capture_read_fd)` line) and BEFORE the
`if interactive && let Some(pgid) = first_pid { give_terminal_to(pgid); }` block.
Keep `capture_read_fd.take()` so later references see `None`.

`io::copy(&mut f, *buf)` reads until EOF (all write ends of the capture pipe
closed = the final stage exited / closed fd 1), draining concurrently with the
stages writing. Then `wait_pipeline_raw` reaps the already-exited stages and
computes `$PIPESTATUS` exactly as before.

### Why it is safe
- **Capture ⟹ non-interactive.** `capture_read_fd` is `Some` only for a Capture
  sink, and `interactive = matches!(sink, StdoutSink::Terminal) && !in_subshell &&
  !in_completion` (executor.rs:3505) is therefore `false` whenever
  `capture_read_fd` is `Some`. So the `give_terminal_to(pgid)` and the
  stopped-pipeline (Ctrl-Z) early-return blocks are no-ops in the capture case —
  draining early cannot interfere with terminal handoff, and a captured pipeline
  cannot be Ctrl-Z-stopped mid-drain.
- **Terminal (non-capture) case unchanged.** `capture_read_fd` is `None` there, so
  the relocated block is a no-op → ZERO behavior change for ordinary pipelines.
- **Status unaffected.** The drain reads bytes only; `wait_pipeline_raw` (run
  after) reaps the exited stages and sets `last_status`/`$PIPESTATUS` as today.
- **No double-close.** After the relocated `.take()`, the post-wait code and the
  stopped-path `if let Some(r) = capture_read_fd { close }` both see `None`. (Those
  now-dead references may be removed for clarity — see plan; harmless either way.)

## Scope & must-not-regress
- **Only the relocation.** No change to fd setup, stage spawning,
  `wait_pipeline_raw`, `$PIPESTATUS`, or the single-command/background paths.
- **Ordinary (Terminal-sink) pipelines** — including interactive job control,
  Ctrl-Z stop, `give_terminal_to`, and `set -o pipefail`/`$PIPESTATUS` — are
  untouched (no `capture_read_fd`).
- **Small captured pipelines** keep working (they already drain fine; draining
  earlier is identical).
- **Backgrounded pipelines** (`run_background_sequence`) and **single captured
  commands** (run_command) are different code paths — not touched.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | Relocate the `run_multi_stage` capture-read from after `wait_pipeline_raw` to before it (drain-before-wait). Optionally delete the now-dead post-wait read + the stopped-path capture close. |
| `tests/captured_pipeline_drain_integration.rs` (NEW) | Large/multi-stage captured-pipeline capture + `$PIPESTATUS` + small-pipeline + non-capture-unaffected, timeout-guarded. |
| `tests/scripts/captured_pipeline_drain_diff_check.sh` (NEW) | bash-diff harness. |
| `docs/bash-divergences.md` | DELETE M-119 (Tier-1 2→1) on completion. |

## Testing

1. **Integration `#[test]`s** (`tests/captured_pipeline_drain_integration.rs`) —
   spawn huck with each fragment and a WALL-CLOCK guard (e.g. spawn + a watchdog
   thread that kills after ~10s, asserting completion); assert exact bytes/length:
   - `x=$(seq 1 500000 | cat); echo ${#x}` → `3388894` (was a hang)
   - `x=$(seq 1 200000 | cat | cat); echo ${#x}` → the right length (3-stage)
   - `x=$(seq 1 1000 | cat); echo ${#x}` → `3892` (small, still works)
   - `x=$(seq 1 500000 | wc -l); echo "[$x]"` → `[500000]` (large producer, small
     final output — already worked; must stay working)
   - `$PIPESTATUS` after a captured pipeline: `x=$(false | true); echo
     "${PIPESTATUS[@]}"` matches bash (`0` for the outer command-substitution
     context — verify the exact bash semantics and assert that).
   - non-capture unaffected: `seq 1 100 | wc -l` → `100`.
   (Use a process-kill watchdog so a regression FAILS as a timeout, not an
   infinite hang of the test run.)
2. **Bash-diff harness** `tests/scripts/captured_pipeline_drain_diff_check.sh` —
   byte-identical bash↔huck for the capture cases (sizes chosen to exceed
   64 KB). Run as file-args (`-c`).
3. **Full regression:** entire unit + integration suite and ALL existing harnesses
   green — ESPECIALLY the pipeline / job-control / `$PIPESTATUS` / pipefail / PTY
   suites (`pty_interactive`, `subshell_pipeline_pty`, `subshell_tty_pty`) — the
   relocation must not regress interactive pipelines. clippy clean.
4. **nvm payoff (REQUIRED this time, needs network — the v132 lesson):** in a PTY,
   `source ~/.nvm/nvm.sh` (NOT `~/.bashrc` — creds) then `nvm ls-remote`; confirm
   it COMPLETES (no hang) and lists versions, BEFORE claiming the fix. Also confirm
   the synthetic `x="$(seq 1 500000 | cat)"` completes. If no network, say so and
   rely on the synthetic + the nvm-shaped `$(eval 'seq 1 500000' | cat)` (which
   reproduces the same deadlock locally).

## Edge cases & notes
- A pipeline whose FINAL stage is an in-process builtin/function (forked stage)
  writes to the same capture pipe → same drain mechanism; covered.
- A final stage with its OWN `>file` redirect produces no capture output
  (capture_read_fd may be None or get EOF immediately) → drain is a no-op/instant.
- The drain blocks until the final stage closes its write end; if some MIDDLE
  stage hangs forever the pipeline would hang regardless (true in bash too) — not
  this bug.
- `$(…)` capture sanitizes `&`→`;` (execute_capturing), so no real background
  child escapes; the drain sees a deterministic EOF.
