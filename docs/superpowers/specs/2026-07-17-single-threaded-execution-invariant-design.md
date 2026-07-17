# v309 — enforce single-threaded execution; isolate the forking tests

**Issue:** [#184](https://github.com/jdstanhope/huck/issues/184) — the `huck-engine`
lib test binary intermittently wedges at `--test-threads >= 2` because a test
forks an in-process subshell while other threads run.

**Goal:** turn huck's undocumented "execution is single-threaded" assumption into
an enforced, self-announcing invariant, and stop the test suite from violating it.

---

## The root cause (measured, not assumed)

huck executes subshells, background jobs, and in-process pipeline stages by
`libc::fork()` **without a following `exec`** — the child continues in the same
address space through `run_command` (malloc, `Vec`/`String`, Rust stdio). POSIX
permits only async-signal-safe calls between `fork` and `exec` in a
**multithreaded** process. huck violates this deliberately, and it is correct
today only because the process has exactly one thread — the comment at
`executor.rs:7938` ("huck is single-threaded so this is fine") states the load-
bearing assumption.

The test harness breaks that assumption: it runs ~1800 tests across N threads.
When `executor::tests::subshell_stderr_is_captured` forks its `( echo out; echo
err >&2 )` subshell, the child is a snapshot of the whole process; if any other
thread held the malloc arena or `io::stdout()` lock at the fork instant, the
child deadlocks on its first allocation and the parent `waitpid`s forever.

**Reproduced:** 3 of 8 runs of the lib binary at `--test-threads 4` wedged, every
one with the identical signature — `subshell_stderr_is_captured` plus three
`expand::tests::glob_*` victims blocked behind the shared `CWD_LOCK` the stuck
forker never released.

There is exactly **one** production fork whose child runs shell code in-process:
`fork_and_run_in_subshell` (`executor.rs:7919`), whose child dives into
`run_command`. The two other production `libc::fork()` sites —
`spawn_failed_stage` (`executor.rs:8042`) and `spawn_command_error_stage`
(`executor.rs:8104`) — are async-signal-safe: their children do only
`dup2`/`close`/`libc::write` then `libc::_exit`, alloc-free, so they cannot
deadlock on an inherited lock and need **no** guard. The two remaining
`libc::fork()` sites (`wait_loop.rs:372`, `stream_loop.rs:337`) are test-only and
also `_exit` immediately. So the guard check has a single home.

## Why the obvious fixes are wrong

- **"Drop `CWD_LOCK` before forking"** (the issue's own suggestion): `CWD_LOCK`
  is the *amplifier*, not the cause. Its one accidental benefit is serializing
  the subshell test against the ~40 other `CWD_LOCK` tests. Remove it and the
  subshell forks *more* freely alongside allocating tests — the child deadlock
  gets *more* likely, just without the full-suite cascade. The wedge reproduces
  even with that accidental serialization, proving the deadlock is against the
  broad population of concurrent tests, not the `CWD_LOCK` cohort.
- **Make separate `Engine` instances thread-safe**: a large, multi-front
  re-architecture (fork+exec subshells, per-engine virtualized cwd, single-owner
  signals/job-control/fds). Out of scope. Explicitly declined; this iteration
  keeps huck single-threaded and *enforces* that.

## Design

Three parts. Part 1 is the enforcement and the forcing function; Part 2 uses it
to find and isolate the offending tests; Part 3 makes the invariant legible.

### Part 1 — the concurrency guard

A global counter tracks how many top-level executions are in flight, and a
thread-local tracks how many of those belong to *this* thread. The difference is
"another thread is executing."

```rust
// crate-level, in a small new module (e.g. exec_guard.rs)
static GLOBAL_ACTIVE: AtomicUsize = AtomicUsize::new(0);
thread_local! { static LOCAL_DEPTH: Cell<usize> = const { Cell::new(0) }; }
```

- **RAII guard** around the universal executor entry,
  `executor::execute_with_sink` (`executor.rs:231`). This is the one function
  *every* execution path funnels through: the public API
  (`run_program_in_sinks` → `run_sourced_contents_in_sinks` → per-line
  `process_line_in_sinks` → `execute_with_sink`) **and** the lib unit tests, which
  call `execute_with_sink` directly. Guarding here — rather than at the public
  entry — is what lets the guard see the lib tests and so drive Part 2. On
  construction `GLOBAL_ACTIVE.fetch_add(1)` and `LOCAL_DEPTH += 1`; on drop the
  reverse. The function is re-entrant (nested constructs, `eval`/`source`
  per-line, function bodies), but every re-entry bumps *both* counters on the
  *same* thread, so `GLOBAL == LOCAL` is preserved for a single thread regardless
  of nesting depth — handled correctly by the check below. (The subshell child
  runs its body via `run_command`, not `execute_with_sink`, so it does not
  re-bump; it does not need to — the parent's frame is what is active at the
  fork.)
- **Fork-site check**, immediately before the in-process `libc::fork()` in
  `fork_and_run_in_subshell` (`executor.rs:7932`) — the sole fork whose child
  runs shell code. The two `spawn_*_stage` forks are async-signal-safe
  (`write`+`_exit`) and are deliberately *not* checked:

```rust
if GLOBAL_ACTIVE.load(Ordering::SeqCst) > LOCAL_DEPTH.with(|d| d.get()) {
    panic!("huck: an Engine is executing on another thread while this thread \
            forks an in-process subshell. huck runs subshells by forking \
            without exec, which is memory-unsafe unless the process is \
            single-threaded. Run each Engine on its own thread only when no \
            other Engine is executing concurrently. (issue #184)");
}
```

**Why `GLOBAL > LOCAL` is the exact condition:** `LOCAL_DEPTH` counts this
thread's active `execute_with_sink` frames (≥1 at a fork, more under nesting).
`GLOBAL_ACTIVE` counts all threads'. They are equal iff this thread is the only
one executing — including all same-thread re-entrancy. `GLOBAL > LOCAL` means some
*other* thread is mid-execution, which is exactly "another shell is executing
concurrently on another thread" — the condition the user asked to detect.

**What it detects, precisely.** The guard fires when an in-process subshell fork
coincides with *another thread executing shell code*. That is the user's target
("two shells at once in different threads") and it is what makes concurrent
multi-`Engine` misuse panic instead of deadlock. It is deliberately *not* a proof
that no other thread holds a lock: a fork racing a thread that is allocating
*outside* `execute_with_sink` would not be counted. That residual does not matter
here, because after Part 2 the lib suite contains no in-process fork at all, and
a production embedder's concurrent engines are, by definition, both executing. So
the guard is exact for the condition that matters and honest about its edge.

**Properties:**
- *Production-silent*: a lone engine is always `GLOBAL == LOCAL == 1` at its
  forks, nested subshells included. Never trips.
- *Precise*: fires only when a fork actually coincides with another live
  execution — #184's exact deadlock condition. Non-forking concurrent execution
  (most of the test suite) never reaches the fork check, so it is unaffected.
- *Cheap*: one relaxed-ish atomic add/sub per top-level line, one load per fork.
  Use `SeqCst` for simplicity; this is not a hot inner loop.
- *Always-on* (not `debug_assert`): a clear panic replaces a silent deadlock or a
  corrupted forked child. The panic fires on the offending (forking) thread
  *before* `libc::fork()`, so it unwinds clean shell code — no half-forked state
  — and the innocent owner thread is unharmed.

The child inherits `GLOBAL_ACTIVE`/`LOCAL_DEPTH` at their fork-instant values;
since a fork only proceeds when `GLOBAL <= LOCAL`, the child never starts from a
tripping state, and its own (single-threaded) nested forks stay `GLOBAL == LOCAL`.

The guard covers only the **fork/deadlock** hazard. The cwd, signal/job-control,
and fd-table races (Blockers 2–4 of the multi-engine analysis) cause silent
corruption, not deadlock, and are out of scope — the guard does not detect them,
and the spec says so rather than implying broader safety.

### Part 2 — isolate the forking tests (guard-driven)

With Part 1 in place, run the lib binary at `--test-threads 4` a handful of
times. A test that forks an in-process subshell while another thread is executing
now **panics with the #184 message**, naming itself — the fast, common case. If
in some run the fork happens to coincide with a *non-executing* thread's
allocation, that run still hangs with the old signature, whose "still running"
list also names the forker. Either way the offender is named, not grep-guessed;
run until a clean pass yields no new name.

Each named test moves into a dedicated integration binary,
`crates/huck-engine/tests/subshell_capture.rs`, rewritten against the **public**
`Engine`/`ExecBuilder` API (which exposes stdout and stderr line callbacks, so
subshell output is capturable without the internal `execute_with_sink`). In a
dedicated binary the fork runs with no concurrent sibling execution — production's
condition — so the guard is satisfied and the deadlock cannot occur.

The confirmed member is `subshell_stderr_is_captured`. The guard determines
whether there are others (e.g. an in-process pipeline stage); the move covers
whatever it flags. The neighboring `external_process_*` and `pipeline_stage_*`
tests run `/bin/sh -c …` (fork+**exec** via `std::process::Command`, child execs
immediately) and do **not** reach `fork_and_run_in_subshell`; they are safe and
stay put unless the guard proves otherwise.

The moved tests must assert the same behavior they do today (a subshell's stdout
and stderr are captured to their respective sinks; the merged-order case if it
moves), so coverage is preserved, only relocated.

### Part 3 — make the invariant legible

- Add one authoritative statement of the invariant — huck's execution is
  single-threaded; in-process subshell forks depend on it; `GLOBAL_ACTIVE`
  enforces it; concurrent multi-engine execution panics by design — as a module
  doc on the new guard module and a short section in `docs/architecture.md`.
- Replace the scattered, on-faith "huck is single-threaded so this is fine"
  comments at the fork sites with a one-line reference to the enforced invariant
  ("single-threaded execution invariant — enforced by `exec_guard`; see
  architecture.md"). No comment should imply the engine tolerates concurrent
  execution.

## Testing

- **Guard unit tests** (in the guard module): (a) a lone execution reaches a fork
  path without panicking (`GLOBAL == LOCAL`); (b) same-thread `eval`/`source`
  re-entrancy does not trip it; (c) the positive case — two threads, one holding
  the guard while the other's fork-site check runs — panics, asserted via
  `std::thread::spawn` + `catch_unwind` with a barrier so the overlap is
  deterministic, not timing-dependent.
- **The repro loop is the #184 gate**: `target/debug/deps/huck_engine-*
  --test-threads 4`, run ~8× alone against a frozen binary, must complete every
  time (0 wedges, 0 panics) after Part 2. It wedged 3/8 before. Because the
  guard turns a straggler into a panic rather than a hang, a missed test fails
  loud and fast instead of timing out.
- **The relocated integration binary** passes and asserts the same capture
  behavior: `cargo test -p huck-engine --test subshell_capture --jobs 1 --
  --test-threads 1`.
- Full per-crate lib tests and the bash-diff sweep stay green (this is
  test-harness + a guard on a cold path; no shell behavior changes).

## Rejected alternatives

- **Guard all execution, not just forks.** Would fire on the ~1800 safe
  concurrent non-forking tests. The fork-site check with the `GLOBAL > LOCAL`
  condition is what confines the panic to the genuine hazard.
- **`debug_assert!` the guard.** Off in release, so an embedder's production
  misuse would still deadlock silently. The cost is a single atomic load; keep it
  always-on.
- **Process-thread-count check at fork** (`/proc/self/task`). A test harness
  always has ≥2 threads, so this false-positives in every test binary including
  the isolated one. Rejected.
- **Bare `GLOBAL_ACTIVE > 1` without the thread-local.** False-positives on
  same-thread `eval`/`source` re-entrancy. The `LOCAL_DEPTH` comparison is what
  makes it precise.
