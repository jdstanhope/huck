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
| `crates/huck-engine/tests/forking_execution_serial.rs` | **New.** The relocated subshell capture checks, via the public `Engine` API, as ONE single-threaded `#[test]`. |
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
    // integration binary (`tests/forking_execution_serial.rs`), where a real subshell
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

### Task 3: relocate the 7 forking tests to a single-threaded binary, via behavioral proxies

The guard (Task 2) flagged **7** in-process-forking lib tests, not one. All 7 move to a single-threaded integration binary; the ones asserting internal state are rewritten as public-API behavioral proxies (user-approved). The binary is named `forking_execution_serial.rs` (it covers subshells, background jobs, coproc, and pipeline stages — not just subshell capture).

**The 7 tests and their homes:**
| test | current file | assertion → behavioral proxy |
|---|---|---|
| `subshell_stderr_is_captured` | `executor/tests.rs` | subshell stdout/stderr captured separately (direct) |
| `on_stdout_line_pipeline_last_stage` | `engine.rs` | on_stdout_line callback fires for last stage (already public — move verbatim) |
| `fork_and_run_in_subshell_echo_stage_writes_to_pipe` | `executor/tests.rs` | raw fork writes to a pipe → in-process subshell pipeline stage writes through the pipe |
| `background_pure_builtin_does_not_mutate_parent_env` | `executor/tests.rs` | `Shell.get` is None → `& wait; echo [${X-unset}]` is `[unset]` |
| `background_pure_builtin_forks_and_registers_job` | `executor/tests.rs` | `last_bg_pid > 0` → `$! -gt 0` |
| `execute_bg_chain_registers_job` | `executor/tests.rs` | `shell.jobs.count()==1` → `jobs` shows one `[…]` line |
| `valid_coproc_name_still_starts` | `executor/coproc_name_tests.rs` | `last_status()==0`, `coprocs[0].name=="MYCO"` → `$?==0`, `$MYCO_PID > 0` |

**Files:**
- Create: `crates/huck-engine/tests/forking_execution_serial.rs`
- Modify: `crates/huck-engine/src/executor/tests.rs`, `crates/huck-engine/src/executor/coproc_name_tests.rs`, `crates/huck-engine/src/engine.rs` (remove the relocated tests)

**Interfaces:**
- Consumes: the live guard (Task 2); the public `huck_engine::Engine` — `fn new() -> Engine`, `fn capture(&mut self, src: &str) -> Output` where `Output { stdout: String, stderr: String, exit_code: i32 }`, and `fn prepare(&mut self, src: &str) -> ExecBuilder` with `.on_stdout_line(FnMut(&str))` and `.capture()`.
- Produces: a green reproduction gate.

- [ ] **Step 1: Create the integration binary**

Create `crates/huck-engine/tests/forking_execution_serial.rs`:

```rust
//! Single-threaded isolation for tests that fork in-process (issue #184).
//!
//! huck runs subshells, background jobs, coprocesses, and in-process pipeline
//! stages by forking WITHOUT exec — the child continues in-process through
//! run_command (malloc, stdio), which is memory-safe only in a single-threaded
//! process. Under a parallel harness a concurrent thread can hold the
//! malloc/stdout lock at the fork instant and the child deadlocks; the
//! exec_guard turns that into a panic. So these CANNOT be parallel `#[test]`s —
//! each would fork while the others execute and trip the guard. They live here
//! as ONE `#[test]` running sequentially, the sole test in this binary, so no
//! sibling execution overlaps a fork. (Precedent: #90 / tee_inherit.rs;
//! streaming_fd_serial.rs.)
//!
//! Moved from lib #[cfg(test)] modules. Internal-state assertions (Shell.jobs,
//! Shell.get, the raw fork_and_run_in_subshell contract) are rewritten as
//! public-API behavioral proxies — same code paths, observed through Engine.

use huck_engine::Engine;

#[test]
fn forking_execution_scenarios() {
    subshell_stdout_and_stderr_captured_separately();
    nested_subshell_forks_single_threaded_without_tripping_the_guard();
    pipeline_last_stage_dispatches_on_stdout_line();
    subshell_pipeline_stage_writes_through_the_pipe();
    background_assignment_does_not_leak_to_parent();
    background_pure_builtin_sets_bang_pid();
    background_chain_registers_one_job();
    valid_coproc_name_starts_and_publishes();
}

/// executor::tests::subshell_stderr_is_captured — stdout and stderr to separate sinks.
fn subshell_stdout_and_stderr_captured_separately() {
    let mut e = Engine::new();
    let out = e.capture("( echo out; echo err >&2 )");
    assert_eq!(out.stdout, "out\n");
    assert_eq!(out.stderr, "err\n");
}

/// Guard same-thread re-entrancy: a nested subshell forks twice on one thread
/// (GLOBAL == LOCAL at each fork). Must not panic.
fn nested_subshell_forks_single_threaded_without_tripping_the_guard() {
    let mut e = Engine::new();
    let out = e.capture("( ( echo deep ) )");
    assert_eq!(out.stdout, "deep\n");
}

/// engine::tests::on_stdout_line_pipeline_last_stage — the on_stdout_line
/// callback fires for a pipeline's last stage. Moved verbatim (already public).
fn pipeline_last_stage_dispatches_on_stdout_line() {
    let mut lines: Vec<String> = Vec::new();
    let mut e = Engine::new();
    e.prepare("echo hi | tr a-z A-Z")
        .on_stdout_line(|line| lines.push(line.to_string()))
        .capture();
    assert_eq!(lines, vec!["HI"]);
}

/// executor::tests::fork_and_run_in_subshell_echo_stage_writes_to_pipe —
/// behavioral proxy: an in-process subshell stage writes through a real pipe to
/// the next stage, driving fork_and_run_in_subshell with stdout → pipe.
fn subshell_pipeline_stage_writes_through_the_pipe() {
    let mut e = Engine::new();
    let out = e.capture("( echo hi-from-subshell ) | cat");
    assert_eq!(out.stdout, "hi-from-subshell\n");
}

/// executor::tests::background_pure_builtin_does_not_mutate_parent_env — a `&`
/// assignment runs in a forked subshell and must not leak to the parent.
fn background_assignment_does_not_leak_to_parent() {
    let mut e = Engine::new();
    let out = e.capture("HUCK_TEST_BG_ASSIGN=v & wait; echo [${HUCK_TEST_BG_ASSIGN-unset}]");
    assert_eq!(out.stdout, "[unset]\n");
}

/// executor::tests::background_pure_builtin_forks_and_registers_job — a
/// pure-builtin `&` forks and sets $! to a real positive pid.
fn background_pure_builtin_sets_bang_pid() {
    let mut e = Engine::new();
    let out = e.capture("echo hi >/dev/null & [ \"$!\" -gt 0 ] && echo haspid; wait");
    assert_eq!(out.stdout, "haspid\n");
}

/// executor::tests::execute_bg_chain_registers_job — `cmd && cmd &` registers
/// exactly one job. Cleans up the still-running sleep.
fn background_chain_registers_one_job() {
    let mut e = Engine::new();
    let out = e.capture("sleep 30 && true & jobs; kill %1 2>/dev/null; wait 2>/dev/null");
    let job_lines = out.stdout.lines().filter(|l| l.starts_with('[')).count();
    assert_eq!(job_lines, 1, "expected exactly one job; stdout: {:?}", out.stdout);
}

/// executor::coproc_name_tests::valid_coproc_name_still_starts — a valid coproc
/// name starts the coprocess (status 0) and publishes NAME_PID.
fn valid_coproc_name_starts_and_publishes() {
    let mut e = Engine::new();
    let out = e.capture("coproc MYCO { :; }; echo rc=$?; echo pid=$MYCO_PID; wait 2>/dev/null");
    assert!(out.stdout.contains("rc=0"), "coproc should start, status 0; stdout: {:?}", out.stdout);
    let pid_line = out.stdout.lines().find(|l| l.starts_with("pid=")).unwrap_or("pid=");
    let pid: i64 = pid_line.trim_start_matches("pid=").trim().parse().unwrap_or(0);
    assert!(pid > 0, "MYCO_PID should be a positive pid; stdout: {:?}", out.stdout);
}
```

**On the proxies:** each is intended to assert the SAME behavior as the lib test it replaces, observed through the public API. Run each and confirm it passes against huck's ACTUAL output. If huck's real output differs cosmetically (e.g. the `jobs` line format, `$!` availability), adjust the assertion to match huck's genuine behavior — do NOT invent output. But if a proxy fails in a way that reveals a real behavioral divergence (e.g. `$!` unset after a pure-builtin `&`, or the bg assignment leaking to the parent), STOP and report it — that is a real bug, not a test to bend.

- [ ] **Step 2: Run the new integration binary**

Run: `ulimit -v 6000000; cargo test -p huck-engine --test forking_execution_serial --jobs 1 -- --test-threads 1`
Expected: PASS, 1 passed. (Real forks, single-threaded → guard silent — the "no false positive" verification the lib tests could not do.)

- [ ] **Step 3: Remove the 7 relocated tests from the lib**

Delete the 7 `#[test] fn` bodies named in the table above from their three source files (`executor/tests.rs` ×5, `executor/coproc_name_tests.rs` ×1, `engine.rs` ×1). After deleting, if any `use` import or test helper is now unused, remove it — the branch must build warning-clean. Run `cargo build -p huck-engine 2>&1 | grep -c warning` after a `touch` of each edited file; expect 0.

- [ ] **Step 4: THE GATE — the repro loop must be clean**

Rebuild the frozen binary and run 8× at 4 threads, alone (nothing else on the box):

```bash
cargo test -p huck-engine --lib --no-run --jobs 1
BIN=$(ls -t target/debug/deps/huck_engine-* | grep -v '\.d$' | head -1)
for i in $(seq 1 8); do
  timeout 90 "$BIN" --test-threads 4 >/dev/null 2>&1 && echo "run $i: OK" || echo "run $i: FAIL/HANG/PANIC rc=$?"
done
```
Expected: `run N: OK` for all 8 (0 wedge, 0 panic). It was 3/8 wedged before. If any run still panics or hangs, the guard found an 8th forker: read its name from `timeout 90 "$BIN" --test-threads 4 2>&1 | grep -E 'issue #184|has been running'`, move it too (back to Step 1's binary), and repeat. Do NOT weaken the guard or the timeout to get green.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/tests/forking_execution_serial.rs crates/huck-engine/src/executor/tests.rs crates/huck-engine/src/executor/coproc_name_tests.rs crates/huck-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
test: isolate the 7 in-process-forking tests to a single-threaded binary (#184)

The exec_guard flagged 7 lib tests that fork in-process (subshells, background
jobs, a coproc, a pipeline stage) while other threads execute — not just the one
found empirically. All move to tests/forking_execution_serial.rs, one #[test]
run sequentially, driven through the public Engine API where the fork runs like
production (single-threaded) and is safe by construction. Internal-state
assertions (Shell.jobs, Shell.get, the raw fork contract) become public-API
behavioral proxies. The lib binary no longer wedges at --test-threads 4 (was
3/8).

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
`crates/huck-engine/tests/forking_execution_serial.rs`. The guard covers only the
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
