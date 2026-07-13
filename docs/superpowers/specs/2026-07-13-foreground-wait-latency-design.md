# v285 — Fix the 100ms foreground-child wait latency

**Issue:** [#120](https://github.com/jdstanhope/huck/issues/120) — Every
foreground external command and subshell costs ~100ms (`stream_loop` no-pipe
sleep). `divergence` + `bug` + `sev:high`.

## Problem

`crates/huck-engine/src/stream_loop.rs::external_capture_loop` is the
foreground-child wait used by the non-interactive / capture / nested-subshell
execution path (`executor.rs:843` for subshells, `executor.rs:6424` for
external commands). It waits by repeatedly calling `waitpid(child, WNOHANG)`
and, between reaps, `wl.poll(100ms tick)` on the capture pipes.

When the child has **no capture pipes** (`pipe_out < 0 && pipe_err < 0` — stdio
is inherited or redirected, which is the normal case for any external command
or subshell whose output is not being captured), the `WaitLoop` has no fds to
watch. On Linux `poll()` with zero fds returns immediately, so the loop hits its
busy-spin guard and calls `std::thread::sleep(poll_tick)` (100ms) before
re-reaping. The child has already exited in microseconds, but the parent has
committed to a full 100ms sleep.

Measured on the current release binary (strace confirms
`wait4(WNOHANG)=0; poll([],0,100); clock_nanosleep(100ms); wait4=reaped`):

| command | huck | bash |
|---|---|---|
| `:` (builtin) | 4ms | — |
| `$( : )` (capture) | 4ms | — |
| `: \| :` (pipeline) | 9ms | — |
| `( : )` (subshell) | **104ms** | 8ms |
| `/bin/true` (external) | **109ms** | — |

The capture path (`$(…)`) and pipelines are fast because pipe-EOF wakes the poll
immediately; only the inherited-stdio path pays the sleep. This is a general
performance bug — any script running external commands is ~100ms/command slower
than bash — and it is the root cause of four bash-test-suite TIMEOUT categories
(`read`, `minimal`, `dollars`, `jobs`), which *complete* just over the 30s
per-category budget (dollars 29s, jobs 43s) rather than truly hanging.

## Root cause detail

`external_capture_loop(child_pid, pipe_out, pipe_err, sinks, timeout_remaining)`:

```rust
loop {
    let wpid = waitpid(child_pid, &mut status, WNOHANG);
    if wpid == child_pid { drain pipes; return status; }
    let to = /* min(timeout_remaining(), poll_tick=100ms) */;
    let events = wl.poll(Some(to))?;
    if pipe_out < 0 && pipe_err < 0 {
        if events.is_empty() { std::thread::sleep(to); }  // <-- the 100ms nap
        continue;
    }
    /* read pipes ... */
}
```

Both call sites pass `|| None` for `timeout_remaining`, so in practice the
no-pipe wait is always also a **no-timeout** wait. (Per-call `ExecBuilder`
timeouts, where they exist, are enforced separately in `timeout.rs`; this
closure is currently always `None`.) The `WaitLoop`/poll machinery exists to
stream captured output in real time — when there is nothing to capture, it has
no purpose and a plain blocking `waitpid` is strictly correct.

## Fix

Add a fast path at the top of `external_capture_loop`: when both pipes are
absent **and** no embedder timeout is active, block directly on the child with
`waitpid(child_pid, 0)`, looping on `EINTR` so signal handlers/traps still run.
Return the child's raw status directly (there are no pipes to drain).

```rust
pub fn external_capture_loop(child_pid, pipe_out, pipe_err, sinks, mut timeout_remaining)
    -> io::Result<i32>
{
    // #120: nothing to stream and no deadline — block on the child instead of
    // the fd-less poll + sleep(tick) loop, which costs ~100ms per call.
    if pipe_out < 0 && pipe_err < 0 && timeout_remaining().is_none() {
        return blocking_wait(child_pid);
    }
    /* ... existing WaitLoop path, unchanged ... */
}

fn blocking_wait(child_pid: libc::pid_t) -> io::Result<i32> {
    loop {
        let mut status = 0;
        let r = unsafe { libc::waitpid(child_pid, &mut status, 0) };
        if r == child_pid { return Ok(status); }
        if r < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) { continue; }
            return Err(err);
        }
        // r == 0 cannot happen without WNOHANG; loop defensively.
    }
}
```

### Why this is behavior-preserving (apart from latency)

- **Signals/traps:** the existing loop already treats `poll` EINTR as
  "continue" (`WaitLoop::poll` maps EINTR → empty events → the loop re-reaps),
  so trap timing is unchanged — `blocking_wait` likewise re-waits on EINTR.
- **No `WUNTRACED`:** the existing loop uses `WNOHANG` **without** `WUNTRACED`;
  `blocking_wait` uses flags `0` (also no `WUNTRACED`), matching it. Foreground
  job-control stop handling lives on the interactive path
  (`wait_with_untraced` / `wait_pipeline_raw`), not here.
- **Reaping ownership:** the caller `std::mem::forget`s the `Child` because the
  loop reaps the pid; `blocking_wait` still reaps it, so this is unchanged.
- **Capture path untouched:** any run with a pipe present takes the existing
  `WaitLoop` path verbatim — streaming callbacks, `$(…)`, `Output` capture, and
  the `timeout_remaining` fallback all behave exactly as before.

The `timeout_remaining() != None` case (currently never hit) keeps the existing
poll+sleep loop, so the timeout machinery stays forward-compatible.

## Testing

1. **Correctness harness** `tests/scripts/external_wait_latency_diff_check.sh` —
   byte-identical bash↔huck output across many external commands and subshells
   (loops of `( : )`, `/bin/true`, subshells writing to stdout, redirected and
   inherited). Standing regression guard in the CI sweep; asserts the fast path
   did not change observable behavior.

2. **Coarse timing integration test**
   `crates/huck-engine/tests/foreground_wait_latency.rs` — drives the public
   `Engine` API with inherited stdout (`.run()`, not `.capture()`, so the
   no-pipe path is exercised): runs ~50 subshells and ~50 external commands and
   asserts each batch completes well under a generous wall-clock ceiling (3s).
   Pre-fix each batch is ~5s (50 × 100ms); post-fix well under 0.5s. The 3s
   ceiling is robust to noise on the 1-core dev box while still failing loudly
   if this exact bug regresses. Its own integration binary (per the
   `tee_inherit.rs` / #90 precedent) so it never shares a process with other
   forking tests.

3. **Suite confirmation (manual, not committed):** after the fix, the `read`,
   `minimal`, `dollars`, `jobs` bash-suite categories drop back under the 30s
   budget (they become normal FAILs on output diffs, not TIMEOUTs).

## Scope / non-goals

- Does **not** touch the capture/streaming path or pipelines.
- Does **not** address the `redir` true deadlock (`<&N-` move-fd operator) —
  that is issue [#121](https://github.com/jdstanhope/huck/issues/121), a
  separate iteration.
- Does **not** refresh `docs/bash-test-suite-baseline.md` (stale) — optional
  follow-up.
