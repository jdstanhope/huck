# #197 Class-A — inward OwnedFd migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans. Steps use `- [ ]` checkboxes.

**Goal:** Retire the interior manual `libc::close` calls (~88 of the 97) by giving each **owned** fd a `std::os::fd::OwnedFd` (or `File`) whose destructor closes it exactly once — so a leaked/double-closed/landed-on-a-freed-0/1/2 fd becomes impossible by construction. This is the Class-A resource-safety half of #197 (the Class-B routing half — the one-model arc — is done).

**Architecture:** Convert the owned-fd sites to RAII; leave genuinely-borrowed `RawFd`s (fd *numbers* like `STDOUT_FILENO`, `dup2` targets, inherited fds) as raw — they are not owned and must not gain a destructor. Chokepoints: `RedirectScope` (saved fds), `make_pipe` (pipe ends), the executor/procsub/stdin_pipe pipe-wiring, and the open helpers (already `File`-backed = OwnedFd).

**Issue:** [#197](https://github.com/jdstanhope/huck/issues/197) (Class-A). **Design:** `docs/superpowers/specs/2026-08-02-fd-one-model-design.md` (Non-goals note the OwnedFd half proceeds independently); the #197 body's "Class A" + "inward OwnedFd migration".

## Global Constraints

- **Behavior-neutral.** No shell-observable change. Guard: full sweep + redirect_audit + engine lib `--test-threads 4` + integration bins + the **`tools/soak` harness** (the fd-leak detector — the primary net for this work).
- Only convert **owned** fds (opened/dup'd/piped, closed exactly once) to `OwnedFd`/`File`. Do NOT wrap borrowed fd numbers (0/1/2, dup2 targets, `as_raw_fd()` peeks) — that would double-close.
- Warning-clean each commit; `cargo fmt --all`; commit trailer.
- Reproduce CI parallelism: `cargo test -p huck-engine --lib -- --test-threads 4`.
- Land as separate task commits (each independently reviewable/revertable). Fd-ownership bugs are subtle — small steps.

---

### Task 1: `RedirectScope` saved fds → `OwnedFd`

**Files:** `crates/huck-engine/src/executor.rs` (~705-840).

- [ ] **Step 1:** Change `saved: Vec<(RawFd, RawFd)>` → `saved: Vec<(RawFd, Option<OwnedFd>)>` (target fd number + the owned dup of the original, `None` when the target was unopened, the current `-1` sentinel).
- [ ] **Step 2:** `redirect()`: `let saved = libc::dup(target_fd)`; if `saved >= 0` wrap `Some(OwnedFd::from_raw_fd(saved))` else `None`. On the dup2-failure path, the `OwnedFd` drops (closes) automatically — delete the manual `libc::close(saved)`.
- [ ] **Step 3:** `close_target()`: same — the saved dup becomes `Option<OwnedFd>`.
- [ ] **Step 4:** `Drop`: for `Some(owned)` → `libc::dup2(owned.as_raw_fd(), target_fd)` then let `owned` drop (closes); for `None` → `libc::close(target_fd)` (close back to unopened — this target fd is not owned by us, it's a real fd we're resetting, so a raw close is correct). Delete the manual `libc::close(saved)`.
- [ ] **Step 5:** Verify: `cargo test -p huck-engine --lib -- --test-threads 4`; a redirect-heavy script leaves fds 0/1/2 only (`bash -c` a loop of `>f`/`2>&1`/`>&3` then count `/proc/$$/fd`). Commit: `refactor(#197): RedirectScope saved fds are OwnedFd (Class-A)`.

---

### Task 2: `make_pipe` returns owned ends

**Files:** `crates/huck-engine/src/child_fd.rs` (`make_pipe`), all callers.

- [ ] **Step 1:** Add `make_pipe_owned(cloexec) -> io::Result<(OwnedFd, OwnedFd)>` (read, write) built on the existing `pipe2`/`fcntl` logic. Keep `make_pipe` (raw) during migration, or switch callers over one at a time.
- [ ] **Step 2:** Migrate the pipe-creation sites that OWN both ends until handed off — the capture/comsub pipe in `capture_via_fork` (executor.rs), procsub pipes (`procsub.rs`), stdin pipe (`stdin_pipe.rs`), pipeline inter-stage pipes (`spawn_pipeline`/wiring). Each `.into_raw_fd()` only at the true ownership-transfer boundary (handing an end to `ChildFd::owned_raw` / a child); the parent-kept end stays `OwnedFd` and drops after use — retiring its manual close.
- [ ] **Step 3:** Verify per site with `--test-threads 4` + the relevant integration bins (procsub, pipeline, capture). Commit per coherent group: `refactor(#197): <site> pipe ends are OwnedFd (Class-A)`.

---

### Task 3: Sweep remaining owned-fd closes

**Files:** `executor.rs`, `fd_writer.rs`, `wait_loop.rs`, `exec_builder.rs`, `stdin_pipe.rs`, `procsub.rs`, `shell_state.rs`.

- [ ] **Step 1:** For each remaining `libc::close`, classify: OWNED (opened/dup'd here, closed once) → convert to `OwnedFd`/`File` + drop; BORROWED (fd number, dup2 target, inherited) → leave raw with a one-line `// borrowed: not owned` note if non-obvious. `fd_writer.rs`/`exec_builder.rs`'s `FdRestore`/`Fd2Restore` guards already RAII the saved fds — fold any stragglers into the same pattern.
- [ ] **Step 2:** Prefer a CLOEXEC-by-default open/dup helper where a new hand-opened fd could leak into a child.
- [ ] **Step 3:** Target: interior manual `libc::close` count (excluding tests) drops from ~88 toward the irreducible borrowed set; `RawFd` count falls. Commit: `refactor(#197): retire remaining owned-fd manual closes (Class-A)`.

---

### Task 4: Full verification + soak + docs

- [ ] **Step 1:** Build both binaries; sweep 0-failed; redirect_audit 0-diverge; engine lib `--test-threads 4`; integration bins; bash-suite PASS-set unchanged.
- [ ] **Step 2:** **Run the `tools/soak` harness** (`tools/soak/run_soak.sh` + `analyze.sh`) — the resting fd/job floor must NOT rise (no new leak). This is the decisive Class-A check.
- [ ] **Step 3:** Update the design doc / #197: Class-A OwnedFd migration landed; record the residual borrowed-RawFd count. Commit: `docs(#197): Class-A OwnedFd migration landed`.

## Self-Review

- **Behavior-neutral:** every task guarded by the full net + soak; only owned fds gain destructors, borrowed stay raw (the double-close trap).
- **Incremental:** RedirectScope → make_pipe → sweep, each its own commit; RAII guards (`FdRestore`) already exist as the pattern.
