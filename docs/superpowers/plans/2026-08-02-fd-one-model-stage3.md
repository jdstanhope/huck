# fd one-model — Stage 3 (delete the software sink) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reach the one-model end state: the interpreter consults exactly one model of "where output goes" (the real fd table). Delete `StdoutSink`/`StderrSink`/`Merged`/`LineDispatchWriter`/`stream_loop`/`execute_capturing`, strip the `&mut StdoutSink`/`&mut StderrSink` parameter from every function (~63 stdout sites + the stderr twins), and route all writes to real fd 1/2. Behavior-neutral: no shell-observable change.

**Architecture:** Production is already one-model after Stage 1/2 — the only live non-`Terminal` use is `run()`'s `merge_stderr` (`StderrSink::Merged`). Task 1 converts that to a real `dup2`. The remaining `Capture` uses are all test-only (`execute_capturing`, the `#[cfg(test)]` `capture()`, the `#[cfg(test)]` comsub path). Task 2 replaces the `Capture` sink *variant* with a `#[cfg(test)]` **thread-local capture buffer** consulted by the two writer-construction chokepoints (`err_writer` for stderr + the stdout-writer twin around executor.rs:107) and by `FdWriter`, so the fast in-process unit tests keep working, parallel-safe (thread-local, no global-fd redirect). With `Capture`/`Merged` gone the sinks are single-variant `Terminal`; Task 3 deletes the types and the now-vestigial parameter.

**Tech Stack:** Rust (huck-engine). Verification net: `cargo test -p huck-engine --lib -- --test-threads 4`, integration bins, `run_diff_checks.sh`, `redirect_audit.sh`, bash-suite.

**Issue:** [#197](https://github.com/jdstanhope/huck/issues/197). **Design:** `docs/superpowers/specs/2026-08-02-fd-one-model-design.md`.

## Global Constraints

- **Behavior-neutral.** No shell-observable change. The guard is the full net (sweep + audit + engine lib under `--test-threads 4` + integration bins + bash-suite), unchanged vs main at every task.
- Warning-clean at every commit (delete dead code, do not `#[allow]`).
- Reproduce CI parallelism locally: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4` (the fd/thread hazards only fire on >1 thread).
- Commit trailer + `cargo fmt --all` per commit.
- This is a large mechanical refactor — land it as **separate task commits** so each is independently reviewable and revertable.

---

### Task 1: `run()` merge → real `dup2` (removes the last production `Merged`)

**Files:** `crates/huck-engine/src/exec_builder.rs`

- [ ] **Step 1:** In `run()`, replace the `StderrSink::Merged` branch with a real fd-level merge: under `self.merge`, `dup2(1, 2)` (save fd 2 first, restore after) around the run with `StderrSink::Terminal`; or reuse the existing redirect machinery. Non-merge is already `Terminal`. After this, `run()` never constructs `Merged`.
- [ ] **Step 2:** Verify: `run()`'s merge case still sends stderr to fd 1 (a small serial integration test, or extend `capture_tempfile_serial`), and `redirect_audit` stays 0-diverge.
- [ ] **Step 3:** Commit: `feat(#197): run() merge via real dup2 — production is now single-model (Stage 3)`.

---

### Task 2: `#[cfg(test)]` thread-local capture; delete `Capture`/`Merged` variants

**Files:** `executor.rs` (`err_writer` + the stdout-writer twin, `FdWriter`, the external-under-capture pipe path ~644-820, `execute_capturing`), `expand.rs` (`capture_command_output`), `exec_builder.rs` (`#[cfg(test)]` `capture()`), `stream_loop.rs`, `executor/tests.rs`.

- [ ] **Step 1: Add a `#[cfg(test)]` thread-local capture.** A module (e.g. `capture_test_hook`) with `thread_local! { static CAP: RefCell<Option<Vec<u8>>> }`, `with_capture(f) -> Vec<u8>` (installs a buffer for the closure, returns it), and `push(bytes)` (append if active). Thread-local → each libtest thread captures independently, no global-fd redirect, no libtest collision.
- [ ] **Step 2: Hook the writer chokepoints.** In the stdout writer construction (around executor.rs:107) and `err_writer`, and in `FdWriter`'s write path, when `#[cfg(test)]` and a capture is active, append to the thread-local buffer instead of writing fd 1/2. In production (`cfg(not(test))`) these hooks compile out entirely.
- [ ] **Step 3: Reroute the test-only capture callers** to `with_capture`: `execute_capturing` (executor/tests.rs's 29 callers keep their signature — rewrite `execute_capturing` to run the sequence with `Terminal` sinks under `with_capture`), the `#[cfg(test)]` `capture_command_output` (comsub-in-tests), and the `#[cfg(test)]` `capture()`. All run the body in-process with real-fd writers that the thread-local intercepts. (Comsub bodies with an external command fork+exec — async-signal-safe, no guard; only subshell forks trip the guard, which these in-process test paths avoid.)
- [ ] **Step 4: Delete the `Capture` variants + machinery.** Remove `StdoutSink::Capture` / `StderrSink::Capture` / `StderrSink::Merged`, `LineDispatchWriter`, the external-under-capture pipe branch (executor.rs ~644-820 — dead once no `Capture` sink reaches it; a forked comsub already handles its own pipe), and `stream_loop.rs` if now unused. `StdoutSink` is now `{ Terminal }`, `StderrSink` is now `{ Terminal }`.
- [ ] **Step 5: Verify** — build warning-clean; `cargo test -p huck-engine --lib -- --test-threads 4` all pass; the 29 `execute_capturing` tests + comsub tests green.
- [ ] **Step 6: Commit** — `refactor(#197): thread-local test capture; delete Capture/Merged sink variants (Stage 3)`.

---

### Task 3: Delete the sink types + strip the parameter

**Files:** `executor.rs`, `builtins.rs`, `expand.rs`, `shell.rs`, and every function carrying `sink`/`err_sink`.

- [ ] **Step 1:** With both sinks single-variant `Terminal`, replace every `sink: &mut StdoutSink` / `err_sink: &mut StderrSink` parameter with nothing (remove it), and every writer built from them with a direct writer: stdout → `FdWriter`(fd 1)/`io::stdout()`, stderr → `io::stderr()`. `err_writer(err_sink, sink)` becomes `err_writer()` returning `Box::new(io::stderr())` (or is inlined). Do this chokepoint-first (`err_writer`, `redir_open_error`, `err_thread_local`) then propagate outward through call sites; the compiler drives the removal.
- [ ] **Step 2:** Delete the `StdoutSink`/`StderrSink` enums and their `use`s/`pub` re-exports (`lib.rs`, `exec_builder.rs` public surface — the `Output` API is unchanged).
- [ ] **Step 3:** `cargo build -p huck` warning-clean; fix each site the compiler flags. This is the bulk of the mechanical work — iterate to zero errors/warnings.
- [ ] **Step 4: Verify** — engine lib `--test-threads 4`, integration bins, full sweep, redirect_audit, bash-suite — all green / unchanged.
- [ ] **Step 5: Commit** — `refactor(#197): remove the StdoutSink/StderrSink parameter and types (Stage 3)`.

---

### Task 4: Full verification + docs + close the arc

- [ ] **Step 1:** Build both binaries; full sweep 0-failed; `redirect_audit` 0-diverge; engine lib `--test-threads 4`; all integration bins; bash-suite PASS-set unchanged vs main.
- [ ] **Step 2:** `git grep StdoutSink; git grep StderrSink; git grep execute_capturing; git grep LineDispatchWriter` → all empty (fully deleted). One model remains.
- [ ] **Step 3:** Update the design doc: Stage 3 landed, arc COMPLETE — the interpreter consults exactly one model of where output goes; `#197`'s incremental `OwnedFd` half can proceed independently.
- [ ] **Step 4: Commit** — `docs(#197): Stage 3 landed — software sink deleted, one-model arc complete`.

## Self-Review

- **Spec coverage:** Task 1 = the last production `Merged`; Task 2 = the test-only `Capture` (thread-local, keeps fast tests); Task 3 = the parameter + type deletion (the "Full" ask); Task 4 = verify + close. Covered.
- **Behavior-neutral:** every task guarded by the full net; the thread-local capture reproduces the old `Capture`-sink output for tests; production writers are the same real fds builtins already use (`FdWriter`, v308).
- **Risk:** Task 3 is large but compiler-driven; Task 2's thread-local avoids a 100-test serial migration and the libtest global-fd collision.
