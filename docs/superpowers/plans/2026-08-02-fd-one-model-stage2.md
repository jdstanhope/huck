# fd one-model — Stage 2 (temp-file capture; drop streaming) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `ExecBuilder` embedder boundary sink-free: rewrite `capture()` to redirect the process's fd 1/2 to a temp file (real fds), run, and read back; and **remove** the streaming-callbacks path (`on_stdout_line`/`on_stderr_line` and its machinery) — no API users exist, and it is the one embedder use a temp file cannot serve (see the design's Stage-2 decision). After Stage 2, nothing at the embedder boundary constructs a `Capture`/`Merged` sink; the only remaining `Capture` user is `execute_capturing` (comsub-in-tests), which Stage 3 removes.

**Architecture:** `capture()` becomes an fd-redirect-to-tempfile scope wrapping the existing stdin/cwd/restricted/timeout run composition (`run_with_sinks_inner`) with **Terminal** sinks. The streaming feature (public `on_stdout_line`/`on_stderr_line`, `Callbacks`, `push_stdout`/`push_stderr`/tee, `run_with_sinks_tee`, `callbacks_thread_local`, `stream_loop`'s callback firing, `LineDispatchWriter`'s callback notify) is deleted; `run()`'s no-callback fast path (fd 1/2 inherit) stays.

**Tech Stack:** Rust (huck-engine); the `tempfile` crate (already a dependency); engine unit tests + `run_diff_checks.sh` + `redirect_audit.sh` + the bash test-suite as the net.

**Issue:** [#197](https://github.com/jdstanhope/huck/issues/197). **Design:** `docs/superpowers/specs/2026-08-02-fd-one-model-design.md` (Stage 2 decision).

## Global Constraints

- No behavior change to shell semantics — this is an embedder-API refactor. The `capture()`/`run()` results for any script must be unchanged (verify via the engine unit tests that use `.capture()` — 64 call sites in `engine.rs`).
- `capture()` must be fork-safe and single-threaded (no drain thread): temp file, not pipe.
- Streaming removal is a clean deletion — do NOT leave dead stubs. Remove the public API, the machinery, and the tests that exercise it (~33 `on_stdout_line`/`on_stderr_line` refs in `engine.rs` + the `streaming_fd_serial` integration binary). Record the removal in the design doc.
- `stream_loop.rs` / `LineDispatchWriter`'s **capture-buffer append** may still be needed by `execute_capturing` (the internal comsub capture kept for tests). Remove only the **callback-firing** parts in Stage 2; keep whatever `execute_capturing` still needs until Stage 3. If `execute_capturing` does not use `stream_loop` at all, note that.
- Per-crate tests only (OOM): `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4` (reproduce CI parallelism). Build the binary with `cargo build -p huck`. Guard sweeps with `ulimit -v 8000000`.
- Commit trailer on every commit; `cargo fmt --all` before each.

---

### Task 1: Temp-file `capture()`

**Files:**
- Modify: `crates/huck-engine/src/exec_builder.rs`

**Interfaces:**
- `capture(self) -> Output` — unchanged signature and semantics; internals switch from `Capture` sinks to an fd-1/2→tempfile redirect + Terminal sinks + read-back.

- [ ] **Step 1: Write a temp-file capture helper.** Add a private helper that:
  1. creates one `tempfile::NamedTempFile` (merge) or two (non-merge);
  2. saves the real fd 1 (and fd 2) via `libc::dup` (record the saved raw fds; on any error, restore + return);
  3. `libc::dup2(tempfile_fd, 1)` (and, non-merge, `dup2(tempfile2_fd, 2)`; under merge, `dup2(tempfile_fd, 2)` so both streams land in the one file in program order);
  4. runs the script through the EXISTING composition with **Terminal** sinks — reuse `run_with_sinks_inner`'s stdin/cwd/restricted/timeout logic (refactor it to accept `StdoutSink::Terminal`/`StderrSink::Terminal`, or extract a `run_core(&cell, …, out, err)` both paths call);
  5. flushes (`io::stdout().flush()`), restores fd 1/2 from the saved dups (`dup2` back, then `close` the saved), and
  6. reads the temp file(s) into `Output { stdout, stderr, exit_code }`. `NamedTempFile` auto-deletes on drop.

- [ ] **Step 2: Point `capture()` at the helper.** Replace `capture()`'s `Capture`-sink body with a call to the helper. Keep the `merge` behavior (both streams → one file).

- [ ] **Step 3: Build + run the capture tests.**
```bash
cargo build -p huck
ulimit -v 8000000
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 2>&1 | tail -3
```
Every `.capture()`-based engine test (there are 64 call sites) must pass, including merge, stderr separation, restricted, cwd, stdin, and timeout cases. If a test's expected output depended on the OLD in-memory ordering vs. the temp file's program-order, investigate against bash before changing it.

- [ ] **Step 4: Verify no fd leak / correct restore.** Add or run a check that `capture()` leaves fds 0/1/2 intact and no temp files linger (a quick loop: 50 `.capture()` calls, assert the process fd count is stable).

- [ ] **Step 5: Commit** — `cargo fmt --all`; `feat(#197): temp-file ExecBuilder::capture() (Stage 2)`.

---

### Task 2: Remove the streaming-callbacks feature

**Files:**
- Modify: `crates/huck-engine/src/exec_builder.rs`, `engine.rs`, `lib.rs`, `executor.rs`, `stream_loop.rs`
- Delete (if fully unused after removal): `crates/huck-engine/src/callbacks_thread_local.rs`, `crates/huck-engine/tests/streaming_fd_serial.rs`

- [ ] **Step 1: Remove the public API.** Delete `ExecBuilder::on_stdout_line` / `on_stderr_line` and the `on_stdout_line`/`on_stderr_line` builder fields.

- [ ] **Step 2: Remove the machinery.** Delete `Callbacks`, `push_stdout`/`push_stderr`/`push_final`, the tee fields and `tee_*_fd`, `run_with_sinks_tee`, and the tee/callback params of `run_with_sinks_inner`. In `run()`, delete the slow path (callbacks) and keep the **fast path** (fd 1/2 inherit / merge-to-fd1). Remove `callbacks_thread_local` (module + `lib.rs` export) if nothing else references it. In `stream_loop.rs` remove the `with_callbacks` firing; keep only what `execute_capturing`'s external-capture drain still needs (or delete `stream_loop` if it becomes unused — verify with a reference search). In `executor.rs`, remove `LineDispatchWriter`'s callback-notify path; keep its capture-buffer append for `execute_capturing`.

- [ ] **Step 3: Remove the streaming tests.** Delete the `engine.rs` unit tests that call `on_stdout_line`/`on_stderr_line` (~33 refs) and the `streaming_fd_serial` integration binary. Do NOT delete `.capture()` or fast-path `run()` tests.

- [ ] **Step 4: Build warning-clean + full test pass.**
```bash
cargo build -p huck 2>&1 | grep -E 'warning|error'   # expect none
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 2>&1 | tail -3
```
Resolve any newly-dead-code warnings by deleting the dead code (not `#[allow]`).

- [ ] **Step 5: Commit** — `refactor(#197): remove ExecBuilder streaming callbacks (Stage 2; revisit later)`.

---

### Task 3: Full verification + docs

**Files:**
- Modify: `docs/superpowers/specs/2026-08-02-fd-one-model-design.md` (mark Stage 2 landed)

- [ ] **Step 1: Build both binaries** — `cargo build --locked -p huck && cargo build --release --locked -p huck`.
- [ ] **Step 2: Full sweep** — `ulimit -v 8000000; tests/scripts/run_diff_checks.sh` → 0 failed.
- [ ] **Step 3: `redirect_audit`** — 0 DIVERGE.
- [ ] **Step 4: Integration bins + bash-suite** — run each `tests/*.rs` and `crates/huck-engine/tests/*.rs` integration binary single-threaded (skip the deleted `streaming_fd_serial`); then the bash-suite runner (`HUCK_BIN=release`, `BASH_SOURCE_DIR`) — PASS-set unchanged vs main.
- [ ] **Step 5: Confirm no `Capture`/`Merged` at the boundary** — grep `exec_builder.rs` for `StdoutSink::Capture`/`StderrSink::Capture`/`StderrSink::Merged`; the only remaining `Capture` construction in the crate should be in `execute_capturing` (+ its tests). Note it for Stage 3.
- [ ] **Step 6:** Update the design doc: Stage 2 landed (temp-file capture, streaming removed). Commit: `docs(#197): Stage 2 landed — temp-file capture, streaming removed`.

## Self-Review

- **Spec coverage:** Task 1 = "temp-file capture()"; Task 2 = "remove streaming (decided C)"; Task 3 = verification + the Stage-3 handoff note. Covered.
- **No placeholders:** the temp-file mechanism (dup-save → dup2 tempfile → run Terminal → restore → read) and the exact removal list are concrete.
- **Behavior-neutral for shells:** the constraint + the 64 `.capture()` tests are the guard; streaming removal deletes an unused feature, not shell behavior.
- **Fork-safety:** temp file, not pipe/thread — preserves the single-threaded-fork invariant.
