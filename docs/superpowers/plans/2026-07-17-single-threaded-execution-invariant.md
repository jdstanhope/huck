# v309 — Single-Threaded Execution Invariant Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** turn huck's undocumented "execution is single-threaded" assumption into an enforced, self-announcing invariant, and stop the test suite from violating it (#184).

**Architecture:** A tiny `exec_guard` module counts active executions globally and per-thread. An RAII marker at `execute_with_sink` (the universal executor entry) maintains the counts; a check just before the one in-process subshell fork panics — instead of deadlocking — when another thread is executing. The panic then drives moving the offending lib test(s) into a single-threaded integration binary.

**Tech Stack:** Rust, `libc`, an `AtomicUsize` + `thread_local!` `Cell`, the public `Engine`/`Output` API, bash-diff harnesses.

**Spec:** `docs/superpowers/specs/2026-07-17-single-threaded-execution-invariant-design.md` — read it first.

**Issue:** [#184](https://github.com/jdstanhope/huck/issues/184).

## Global Constraints

- **Branch:** `v309-single-threaded-execution`. Never commit to `main`; never merge.
- **Commit trailer**, every commit, exactly: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **`cargo fmt --all`** before every commit — CI enforces `cargo fmt --all --check`. Note `rustfmt` `reorder_modules` is on (default), so a new `mod` line lands alphabetically regardless of where you type it.
- **⚠️ NEVER run `cargo test --workspace` or a bare `cargo test`** — 1 core / 1.9 GB box; it OOM-kills the session. Per-crate: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck`.
- **The reproduction gate** is the frozen lib test binary at `--test-threads 4`, run ~8× **alone** (nothing else on the box, or timings lie): `timeout 90 target/debug/deps/huck_engine-<hash> --test-threads 4`. It wedged 3/8 before this work; it must be clean after Task 3.
- **The guard is always-on** (not `debug_assert`), fires only on `GLOBAL_ACTIVE > LOCAL_DEPTH`, and is silent for a lone engine (`GLOBAL == LOCAL`).
- **Only `fork_and_run_in_subshell` is checked.** The `spawn_failed_stage`/`spawn_command_error_stage` forks are async-signal-safe (`write`+`_exit`) and must NOT get the check.
- **Panic message** must name issue #184 and explain the fork/exec/single-thread reason (exact text in Task 1).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/huck-engine/src/exec_guard.rs` | **New.** The invariant: `ExecActive` RAII counter + `assert_single_threaded_fork()` + the module doc that is the authoritative statement of the invariant. |
| `crates/huck-engine/src/lib.rs` | Register the module. |
| `crates/huck-engine/src/executor.rs` | Hold an `ExecActive` across `execute_with_sink`; call the check before the fork in `fork_and_run_in_subshell`; replace the on-faith comment. |
| `crates/huck-engine/src/executor/tests.rs` | Remove the relocated forking test(s). |
| `crates/huck-engine/tests/subshell_capture.rs` | **New.** The relocated subshell capture checks, via the public `Engine` API, as ONE single-threaded `#[test]`. |
| `docs/architecture.md` | A short "Single-threaded execution" section. |

---

### Task 1: the `exec_guard` module

**Files:**
- Create: `crates/huck-engine/src/exec_guard.rs`
- Modify: `crates/huck-engine/src/lib.rs` (register the module)

**Interfaces:**
- Consumes: nothing.
- Produces — Task 2 depends on these exact signatures:
  - `pub(crate) struct ExecActive` with `pub(crate) fn enter() -> ExecActive` and a `Drop` impl.
  - `pub(crate) fn assert_single_threaded_fork()`.

- [ ] **Step 1: Write the module with its test**

Create `crates/huck-engine/src/exec_guard.rs`:

```rust
//! Single-threaded-execution invariant (issue #184).
//!
//! huck runs subshells, background jobs, and in-process pipeline stages by
//! forking WITHOUT a following `exec`: the child continues in the same address
//! space through `run_command` (malloc, `Vec`/`String`, Rust stdio). POSIX
//! permits only async-signal-safe calls between `fork` and `exec` in a
//! MULTITHREADED process, so this is memory-safe only while the process is
//! single-threaded — which huck is, in production. (The only production fork
//! whose child runs shell code is `executor::fork_and_run_in_subshell`; the
//! `spawn_*_stage` forks do `write`+`_exit` and are async-signal-safe.)
//!
//! This module makes that invariant explicit and enforced. `ExecActive` counts
//! how many executions are in flight — globally, and on this thread.
//! `assert_single_threaded_fork` runs just before the in-process fork; if
//! another thread is executing (`GLOBAL_ACTIVE > LOCAL_DEPTH`), it PANICS with a
//! clear message instead of letting the forked child deadlock on a lock the
//! other thread holds. In a single-threaded process the two counts are equal and
//! the check is a no-op.
//!
//! See `docs/architecture.md` ("Single-threaded execution").

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

static GLOBAL_ACTIVE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static LOCAL_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// RAII marker: an execution is active on this thread while it lives. Construct
/// one at the top of `execute_with_sink`. Re-entrant — nested executions on the
/// same thread each hold their own, and both counters move together, so
/// `GLOBAL_ACTIVE` and this thread's `LOCAL_DEPTH` stay equal for a lone thread
/// regardless of nesting.
pub(crate) struct ExecActive {
    _priv: (),
}

impl ExecActive {
    pub(crate) fn enter() -> Self {
        GLOBAL_ACTIVE.fetch_add(1, Ordering::SeqCst);
        LOCAL_DEPTH.with(|d| d.set(d.get() + 1));
        ExecActive { _priv: () }
    }
}

impl Drop for ExecActive {
    fn drop(&mut self) {
        LOCAL_DEPTH.with(|d| d.set(d.get() - 1));
        GLOBAL_ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Panic if an in-process subshell fork is about to happen while ANOTHER thread
/// is executing shell code. Call immediately before `libc::fork()` in
/// `fork_and_run_in_subshell`. No-op in a single-threaded process (production,
/// and any correctly-isolated test): `GLOBAL_ACTIVE == LOCAL_DEPTH`.
pub(crate) fn assert_single_threaded_fork() {
    let global = GLOBAL_ACTIVE.load(Ordering::SeqCst);
    let local = LOCAL_DEPTH.with(|d| d.get());
    if global > local {
        panic!(
            "huck: an Engine is executing on another thread while this thread \
             forks an in-process subshell. huck runs subshells by forking \
             without exec, which is memory-unsafe unless the process is \
             single-threaded. Run each Engine on its own thread only when no \
             other Engine is executing concurrently. (issue #184)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Barrier};
    use std::thread;

    // NOTE: a "no panic when lone" assertion is NOT reliable in this
    // multithreaded lib binary — a concurrent test's `ExecActive` inflates
    // `GLOBAL_ACTIVE`. That direction is verified in the single-threaded
    // integration binary (`tests/subshell_capture.rs`), where a real subshell
    // forks without panicking. Here we test only the PANIC direction, which is
    // robust: forcing another thread to hold an `ExecActive` guarantees
    // `GLOBAL_ACTIVE > LOCAL_DEPTH` no matter what else runs.
    #[test]
    fn fork_while_another_thread_executes_panics() {
        let start = Arc::new(Barrier::new(2));
        let release = Arc::new(AtomicBool::new(false));
        let (s2, r2) = (start.clone(), release.clone());
        let other = thread::spawn(move || {
            let _active = ExecActive::enter();
            s2.wait(); // both threads meet here; the other now holds _active
            while !r2.load(Ordering::SeqCst) {
                thread::yield_now();
            }
        });
        start.wait();
        // This thread is executing too (LOCAL_DEPTH = 1); GLOBAL_ACTIVE >= 2.
        let _mine = ExecActive::enter();
        // Expected: one panic message printed to stderr; it is caught here.
        let caught = std::panic::catch_unwind(assert_single_threaded_fork);
        assert!(
            caught.is_err(),
            "a fork while another thread executes must panic"
        );
        release.store(true, Ordering::SeqCst);
        other.join().unwrap();
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/huck-engine/src/lib.rs`, add (rustfmt will alphabetize it; `exec_guard` sorts after `exec_builder` on line 27 and before `executor` on line 28):

```rust
pub(crate) mod exec_guard;
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p huck-engine --jobs 1 --lib exec_guard -- --test-threads 1`
Expected: PASS, 1 passed. One panic message ("huck: an Engine is executing…") prints to stderr — that is the caught panic, not a failure.

- [ ] **Step 4: Verify the test is not vacuous**

Temporarily change the check to `if global > local + 1` (so the forced `global==2, local==1` no longer trips) and re-run:

Run: `cargo test -p huck-engine --jobs 1 --lib exec_guard -- --test-threads 1`
Expected: `fork_while_another_thread_executes_panics` **FAILS** (no panic caught). Restore `if global > local` and confirm PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/exec_guard.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
feat: exec_guard — enforce single-threaded in-process fork (#184)

huck forks subshells without exec; the child runs shell code in-process, which
is memory-safe only single-threaded. ExecActive counts active executions
(global + per-thread); assert_single_threaded_fork panics when a fork would
happen while another thread executes, instead of letting the child deadlock.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: wire the guard in

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`execute_with_sink` ~line 231; `fork_and_run_in_subshell` ~line 7932 and its child comment ~line 7937)

**Interfaces:**
- Consumes: `crate::exec_guard::ExecActive::enter()`, `crate::exec_guard::assert_single_threaded_fork()` (Task 1).
- Produces: the live invariant. After this task, a lone engine is unaffected; a concurrent forking execution panics.

- [ ] **Step 1: Hold an `ExecActive` across `execute_with_sink`**

In `crates/huck-engine/src/executor.rs`, at the top of `pub fn execute_with_sink` (before the existing `let guard = unsafe { … install_err_sinks_raw … }`), insert:

```rust
    // #184: mark this thread as executing for the duration of this call, so the
    // fork check in `fork_and_run_in_subshell` can tell whether any OTHER thread
    // is executing. Re-entrant (nested constructs, eval/source, function
    // bodies); the counters stay balanced. Dropped last, on scope exit/panic.
    let _exec_active = crate::exec_guard::ExecActive::enter();
```

- [ ] **Step 2: Check before the in-process fork; replace the on-faith comment**

In `fork_and_run_in_subshell`, the current code is:

```rust
    flush_stdout();
    let pid = unsafe { libc::fork() };
```

and the child branch opens with:

```rust
    if pid == 0 {
        // CHILD: async-signal-safe-ish operations only until we dive into
        // `run_command`. huck is single-threaded so this is fine.
```

Change the fork site to check first:

```rust
    // #184: huck runs this subshell by forking WITHOUT exec — the child
    // continues in-process through `run_command`, which is memory-safe only in
    // a single-threaded process (see `exec_guard`). Panic loudly here rather
    // than let the forked child deadlock on a lock another thread holds.
    crate::exec_guard::assert_single_threaded_fork();
    flush_stdout();
    let pid = unsafe { libc::fork() };
```

and replace the child comment's second sentence:

```rust
    if pid == 0 {
        // CHILD: async-signal-safe-ish operations only until we dive into
        // `run_command`. Safe because the single-threaded-execution invariant
        // (enforced by `exec_guard`, checked just above the fork) holds.
```

- [ ] **Step 3: Confirm the lone-engine path is unaffected (single-threaded)**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: all pass, same count as before this branch (the guard is silent at 1 thread; `GLOBAL == LOCAL`). The `exec_guard` test still passes.

- [ ] **Step 4: Confirm the guard is now LIVE (this is expected, not a regression)**

Build the frozen binary and run once at 4 threads:

```bash
cargo test -p huck-engine --lib --no-run --jobs 1
BIN=$(ls -t target/debug/deps/huck_engine-* | grep -v '\.d$' | head -1)
timeout 90 "$BIN" --test-threads 4 2>&1 | grep -E 'panicked|issue #184|subshell_stderr_is_captured' | head
```
Expected: the run now **panics** on `subshell_stderr_is_captured` citing issue #184 (or, in a run where the fork races a non-executing thread, hangs with that test in the "still running" list). **This is by design** — Task 3 removes the offending test. Do not try to make 4-threads clean in this task.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
feat: check the single-threaded invariant before the subshell fork (#184)

execute_with_sink now holds an ExecActive for its duration, and
fork_and_run_in_subshell asserts no other thread is executing before forking.
A lone engine is unaffected. The multithreaded lib suite now panics on the
in-process-forking test instead of deadlocking — that test moves out in the
next commit.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: relocate the forking test(s), guard-driven

**Files:**
- Create: `crates/huck-engine/tests/subshell_capture.rs`
- Modify: `crates/huck-engine/src/executor/tests.rs` (remove the relocated test)

**Interfaces:**
- Consumes: the live guard (Task 2); the public `huck_engine::Engine` with `fn new() -> Engine` and `fn capture(&mut self, src: &str) -> Output`, where `Output { stdout: String, stderr: String, exit_code: i32 }`.
- Produces: a green reproduction gate.

- [ ] **Step 1: Enumerate the offenders with the guard**

Run the frozen 4-thread binary ~8× and collect every test named by a panic (`issue #184`) or by the "still running" hang list:

```bash
BIN=$(ls -t target/debug/deps/huck_engine-* | grep -v '\.d$' | head -1)
for i in $(seq 1 8); do
  timeout 90 "$BIN" --test-threads 4 2>&1 \
    | grep -E 'issue #184|has been running' | sed 's/^/  /'
  echo "--- run $i done ---"
done
```
The confirmed member is `executor::tests::subshell_stderr_is_captured`. Record every distinct test name that appears. If any test OTHER than `subshell_stderr_is_captured` appears, treat it the same way in the steps below (move it too), and note it in your report — the guard is the authority here, not this plan's guess.

- [ ] **Step 2: Create the integration binary**

Create `crates/huck-engine/tests/subshell_capture.rs`:

```rust
//! Single-threaded isolation for subshell output-capture checks (issue #184).
//!
//! huck runs `( … )` subshells by forking WITHOUT exec — the child continues
//! in-process through run_command (malloc, stdio), which is memory-safe only in
//! a single-threaded process. Under a parallel harness a concurrent thread can
//! hold the malloc/stdout lock at the fork instant and the child deadlocks; the
//! exec_guard now turns that into a panic. So these checks CANNOT be parallel
//! `#[test]`s. They live here as ONE `#[test]` running scenarios sequentially —
//! the sole test in this binary, so no sibling execution overlaps a fork and
//! the guard stays silent. (Precedent: #90 / tee_inherit.rs;
//! streaming_fd_serial.rs.)
//!
//! Moved from `executor::tests::subshell_stderr_is_captured`, rewritten against
//! the public `Engine` API. Coverage is preserved, only relocated.

use huck_engine::Engine;

#[test]
fn subshell_capture_scenarios() {
    subshell_stdout_and_stderr_captured_separately();
    nested_subshell_forks_single_threaded_without_tripping_the_guard();
}

/// A subshell's stdout and stderr are captured to their separate sinks.
fn subshell_stdout_and_stderr_captured_separately() {
    let mut e = Engine::new();
    let out = e.capture("( echo out; echo err >&2 )");
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");
}

/// A nested subshell forks twice on one thread — the guard's same-thread
/// re-entrancy path (GLOBAL == LOCAL at each fork). Must not panic.
fn nested_subshell_forks_single_threaded_without_tripping_the_guard() {
    let mut e = Engine::new();
    let out = e.capture("( ( echo deep ) )");
    assert_eq!(out.stdout, "deep\n");
}
```

If Step 1 surfaced additional forking tests, add each as another helper called from `subshell_capture_scenarios`, rewritten via `Engine::capture` to assert the same thing it asserted in the lib.

- [ ] **Step 3: Run the new integration binary**

Run: `ulimit -v 6000000; cargo test -p huck-engine --test subshell_capture --jobs 1 -- --test-threads 1`
Expected: PASS, 1 passed. (The real subshell forks single-threaded, so the guard is silent — this is the "no false positive" verification the lib unit test could not do.)

- [ ] **Step 4: Remove the relocated test from the lib**

In `crates/huck-engine/src/executor/tests.rs`, delete the whole `#[test] fn subshell_stderr_is_captured() { … }` (and any other test Step 1 flagged). If deleting it leaves `use` imports unused (e.g. an import only that test needed), remove those too — the branch must build warning-clean.

- [ ] **Step 5: THE GATE — the repro loop must be clean**

Rebuild the frozen binary and run 8× at 4 threads, alone:

```bash
cargo test -p huck-engine --lib --no-run --jobs 1
BIN=$(ls -t target/debug/deps/huck_engine-* | grep -v '\.d$' | head -1)
for i in $(seq 1 8); do
  timeout 90 "$BIN" --test-threads 4 >/dev/null 2>&1 && echo "run $i: OK" || echo "run $i: FAIL/HANG rc=$?"
done
```
Expected: `run N: OK` for all 8 (0 wedges, 0 panics). It was 3/8 wedged before. If any run still panics or hangs, Step 1 missed a test — read its name from the output, move it (back to Step 2), and repeat. Do NOT weaken the guard or the timeout to get green.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/tests/subshell_capture.rs crates/huck-engine/src/executor/tests.rs
git commit -m "$(cat <<'EOF'
test: isolate the in-process-forking subshell test to its own binary (#184)

The exec_guard flagged executor::tests::subshell_stderr_is_captured as forking
an in-process subshell while other threads execute. Moved to a single-threaded
integration binary (tests/subshell_capture.rs) via the public Engine::capture
API, where the fork runs like production — one thread — so it is safe by
construction. The lib binary no longer wedges at --test-threads 4 (was 3/8).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: document the invariant

**Files:**
- Modify: `docs/architecture.md`
- Modify: `crates/huck-engine/src/executor.rs` (only if Step 1 finds other on-faith comments)

**Interfaces:**
- Consumes: the shipped guard.
- Produces: the authoritative doc the module and fork-site comments reference.

- [ ] **Step 1: Find any other on-faith single-thread comments**

Run: `grep -rn 'single-threaded\|single threaded' crates/huck-engine/src/ | grep -v test`
For each hit, read it. The one at `fork_and_run_in_subshell`'s child was already updated in Task 2. If any other comment *asserts* thread-safety or single-threadedness "so this is fine" without referencing the enforced invariant, change it to a one-line pointer: `// single-threaded execution invariant — enforced by exec_guard; see docs/architecture.md`. Do not touch comments that merely describe async-signal-safety of a specific `write`+`_exit` child (those are accurate and unrelated).

- [ ] **Step 2: Add the architecture.md section**

In `docs/architecture.md`, add a short section (place it under the cross-cutting conventions, near the other execution notes):

```markdown
### Single-threaded execution (invariant, enforced)

huck executes subshells, background jobs, and in-process pipeline stages by
`fork()`ing **without a following `exec`** — the child continues in the same
address space through `run_command`. POSIX allows only async-signal-safe calls
between `fork` and `exec` in a multithreaded process, so this is memory-safe
**only while the process is single-threaded.** huck is, in production.

This is enforced, not assumed. `exec_guard` (`crates/huck-engine/src/exec_guard.rs`)
counts active executions globally and per-thread; `execute_with_sink` holds an
`ExecActive` for its duration, and `fork_and_run_in_subshell` calls
`assert_single_threaded_fork()` before the fork. If another thread is executing,
it **panics** (citing #184) rather than let the forked child deadlock on an
inherited lock. A lone engine never trips it.

Consequences: running two `Engine`s concurrently on different threads is
unsupported and will panic at the first subshell fork. Tests that fork an
in-process subshell must run single-threaded — see
`crates/huck-engine/tests/subshell_capture.rs`. The guard covers only the
fork/deadlock hazard; the cwd, signal/job-control, and fd-table state are also
process-global and would need per-engine virtualization for true multi-engine
support (out of scope; declined in the #184 design).
```

- [ ] **Step 3: Verify docs build/no broken refs**

Run: `grep -rn 'exec_guard\|assert_single_threaded_fork' docs/architecture.md crates/huck-engine/src/exec_guard.rs`
Expected: the architecture.md section and the module doc both reference the guard by its real names (`exec_guard`, `assert_single_threaded_fork`, `ExecActive`).

- [ ] **Step 4: Commit**

```bash
git add docs/architecture.md crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
docs: state the single-threaded execution invariant authoritatively (#184)

Adds an architecture.md section as the one place the invariant lives, referenced
from exec_guard's module doc and the fork-site comments. Replaces any remaining
on-faith "single-threaded so this is fine" comments with a pointer to it.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Verification (controller, before the PR)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo build -p huck --locked` and `cargo build --release -p huck --locked`.
- [ ] `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
- [ ] `cargo test -p huck-engine --test subshell_capture --jobs 1 -- --test-threads 1` — passes.
- [ ] **The repro gate**: the frozen lib binary at `--test-threads 4`, 8× alone, all `OK` (0 wedge, 0 panic). This is the #184 fix, proven.
- [ ] Every `-p huck` integration binary, each single-threaded with a `ulimit -v` guard (behavior unchanged, but confirm no fallout).
- [ ] `tests/scripts/run_diff_checks.sh` on both binaries — green (this is test-harness + a guard on a cold path; no shell behavior changes, so no diff expected).
- [ ] PR with `Closes #184`; **the user merges, not you.** Wait for CI to finish and pass before saying it is ready.
