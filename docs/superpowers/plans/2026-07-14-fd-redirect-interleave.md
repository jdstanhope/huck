# fd redirect application — Approach B Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make in-process redirects apply INTERLEAVED (resolve→apply per redirect against the real fd table, per bash), fixing the 6 `{var}` regressions the Approach-C branch introduced, while the child path keeps its batch plan (retaining the 8 external fixes). Share the per-redirect resolution as `lower_one_redirect`.

**Architecture:** Branch from the C branch `v292-redirect-lowering` (which already has `OwnedFd`/`PlanOp`/`FdPlacement`/`redir_plan_to_child`/`fd_state`/child-batch — all of which B keeps). Extract the per-redirect resolution into `lower_one_redirect` (shared), then replace the in-process batch `apply_plan` with an interleaved `apply_redirects`/`apply_one`. `OwnedFd` stays.

**Tech Stack:** Rust, `std::os::fd::OwnedFd`, libc. Crate: `huck-engine`. All code in `crates/huck-engine/src/executor.rs` + `tests/scripts/fd_torture_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-07-14-fd-redirect-interleave-design.md`. **Evaluation (design basis):** `docs/superpowers/reviews/2026-07-14-fd-management-design-evaluation.md`. **Issue:** #139.

## Global Constraints

- **Behavior-preserving where noted; the ONLY intended behavior change is fixing the 6 in-process `{var}` regressions.** Everything else must stay byte-identical to the C branch (and hence, for non-`{var}`, to bash/main).
- **Acceptance gate — the differential audit.** `tools/redirect_audit.sh` (run `HUCK=<debug binary> bash tools/redirect_audit.sh`). Reference counts: **main = 24 diverge, C branch = 22 diverge.** After Task 2 this branch must show **exactly 16 divergences** — the persistent/orthogonal set — with **NONE** of these 6 labels present:
  `inproc  nf+use {v}>f 2>&$v`, `exec    nf+use {v}>f 2>&$v`, `inproc  nf+fail {v}>f 2>&9`, `exec    nf+fail {v}>f 2>&9`, `inproc  nf+num 3>a {v}>x`, `exec    nf+num 3>a {v}>x`. No NEW label may appear.
- **Commit trailer, verbatim, last line of every commit:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit.
- **Test/build discipline (this box OOMs on `cargo test --workspace`):** build `cargo build -p huck` (debug) / `cargo build --release -p huck`; lib `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`; integration `ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1`; sweep `( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh )`.
- **DO NOT TOUCH:** `RedirectSlot`/`slots_for_simple_path`/`slot_consumes`/`stage_extra_redirects`/`build_child_extra_ops`/the `run_multi_stage`+`run_background_sequence` slot reads (Phase 3b), and the H7 sink layer (`redirs_write_stdout`, `final_dests_for_1_2`, `run_builtin_with_redirects` routing, `force_terminal`).
- **DO NOT try to fix** #137 (write-to-closed-fd), #140 (`{var}` message wording / external `$v`), or #141 (child `{var}` numbering). Those persistent divergences must remain exactly as on the C branch.

---

### Task 0: Create the branch

- [ ] **Step 1: Branch from the C branch**
```bash
git checkout v292-redirect-lowering
git checkout -b v292b-redirect-interleave
```
- [ ] **Step 2: Baseline the audit on this branch (should equal the C branch = 22)**
```bash
cargo build -p huck
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | sed -n '2,3p'
```
Expected: `... 22 DIVERGE`. Record the 22 labels (`... | grep '^DIVERGE:'`) — Task 2 must drop exactly the 6 in-process `{var}` labels to reach 16.

---

### Task 1: Extract `lower_one_redirect` (shared per-redirect resolver) — behavior-preserving

Factor the per-redirection resolution out of the batch `lower_redirects` + `lower_named_fd` into one `lower_one_redirect`, and rebuild the batch `lower_redirects` as a loop over it. BOTH the child path and (still) the in-process batch `apply_plan` consume the batch `lower_redirects`, so behavior is IDENTICAL to the C branch this task (audit stays 22).

**Files:** Modify `crates/huck-engine/src/executor.rs`.

**Interfaces:**
- Produces: `fn validate_source(src: RawFd, fd_state: Option<&std::collections::HashMap<RawFd, bool>>, shell: &mut Shell, sink: &mut StdoutSink, err_sink: &mut StderrSink) -> Result<(), i32>` and `fn lower_one_redirect(redir: &Redirection, shell: &mut Shell, sink: &mut StdoutSink, err_sink: &mut StderrSink, fd_state: Option<&mut std::collections::HashMap<RawFd, bool>>, writers: &mut Vec<libc::pid_t>) -> Result<Vec<PlanOp>, i32>`.
- Consumes (unchanged, from the C branch): `PlanOp`, `RedirPlan`, `open_redirect_file`/`FdPlacement`, `resolve_dup_source`, `check_restricted_redirect`, `expand_single`/`expand_assignment`, `spawn_heredoc_writer`, `relocate_high_cloexec`, `crate::child_fd::dup_to_high_fd`, `redir_open_error`, `validate_fd_open`, `validate_plan_source`.

- [ ] **Step 1: Add `validate_source`** (routes dup validation: `None` = real fds, for the interleaved in-process caller where real fds already reflect earlier applies; `Some(fd_state)` = simulation, for the child batch). Place it next to `validate_plan_source`:
```rust
/// Validate a dup/move source. In-process (interleaved) passes `None` and we
/// check the REAL fd table (earlier redirects are already applied to it). The
/// child (batch) passes `Some(fd_state)` and we defer to the plan simulation.
/// Emits bash's `"{src}: Bad file descriptor"` and returns `Err(1)` if not open.
fn validate_source(
    src: RawFd,
    fd_state: Option<&std::collections::HashMap<RawFd, bool>>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<(), i32> {
    match fd_state {
        Some(state) => validate_plan_source(src, state, shell, sink, err_sink),
        None => validate_fd_open(src, shell, sink, err_sink).map_err(|()| 1),
    }
}
```

- [ ] **Step 2: Add `lower_one_redirect`.** Move the body of the C-branch `lower_redirects` per-iteration `match &redir.op { … }` (the File / Dup|Move / Close / Heredoc / HereString arms) and the whole `lower_named_fd` into ONE function that resolves a SINGLE redirection into `Vec<PlanOp>`, with these changes from the C code:
  - Heredoc/HereString/`{var}`-heredoc: push the writer pid to `writers` (the passed accumulator) instead of `plan.heredoc_writers`.
  - Dup/Move source (both the non-`{var}` arm AND the `lower_named_fd` dup arm): call `validate_source(src, fd_state.as_deref(), …)?` BEFORE emitting the `InstallDup` / before `dup_to_high_fd`. (This replaces the C branch's inline `validate_plan_source` in `lower_redirects` and preserves the `{var}` bad-dup message.)
  - `{var}` File/heredoc/dup resolution and the `dup_to_high_fd(src, 10, false)` allocation stay verbatim from `lower_named_fd`.
  - When `fd_state` is `Some`, update it for the ops produced (see Step 3's helper), so the child's later dups see this redirect's effect. When `None`, do not (the in-process caller validates against real fds).
  - Return `Ok(ops)` (a `Vec<PlanOp>`, 0–2 entries). No `fail!` macro reaping here — the CALLER reaps `writers` on error (Task 1 keeps `lower_redirects`'s reap; Task 2's in-process caller reaps via `scope.reap_heredoc_writers()`).

  Signature:
```rust
fn lower_one_redirect(
    redir: &Redirection,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    mut fd_state: Option<&mut std::collections::HashMap<RawFd, bool>>,
    writers: &mut Vec<libc::pid_t>,
) -> Result<Vec<PlanOp>, i32> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let mut ops: Vec<PlanOp> = Vec::new();
    // ... {var} branch (from lower_named_fd) pushing into `ops`, validating the
    //     dup source via validate_source(.., fd_state.as_deref(), ..), updating
    //     *fd_state (if Some) for the NamedFd high fd (open) + any move Close ...
    // ... else: target = redir.target_fd(); match &redir.op { File|Dup|Move|
    //     Close|Heredoc|HereString } pushing into `ops`, validating dup sources,
    //     updating *fd_state (if Some) for target(open)/close(closed) ...
    Ok(ops)
}
```
  Use the C-branch File/Dup/Close/Heredoc/HereString/`{var}` arms verbatim for the resolution; the only edits are the four bullets above (writers accumulator, `validate_source`, fd_state updates gated on `Some`, no `fail!`).

- [ ] **Step 3: Rebuild `lower_redirects` as a batch loop over `lower_one_redirect`** (this is the CHILD lowering; also still used by the in-process `apply_plan` until Task 2):
```rust
fn lower_redirects(
    redirects: &[Redirection],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<RedirPlan, i32> {
    let mut fd_state: std::collections::HashMap<RawFd, bool> = std::collections::HashMap::new();
    let mut plan = RedirPlan { ops: Vec::new(), heredoc_writers: Vec::new() };
    for redir in redirects {
        match lower_one_redirect(redir, shell, sink, err_sink, Some(&mut fd_state), &mut plan.heredoc_writers) {
            Ok(ops) => plan.ops.extend(ops),
            Err(code) => {
                // Close opened fds first (heredoc read ends -> writer EOF/EPIPE),
                // then reap — hang-free even for >64KB bodies (the C fail! order).
                plan.ops.clear();
                for pid in plan.heredoc_writers.drain(..) {
                    let mut st = 0;
                    unsafe { libc::waitpid(pid, &mut st, 0) };
                }
                return Err(code);
            }
        }
    }
    Ok(plan)
}
```
Delete the old inline `lower_named_fd` (its logic now lives in `lower_one_redirect`) and the old inline loop body. `build_child_redir_plan` (`redir_plan_to_child(lower_redirects(...))`) is unchanged.

- [ ] **Step 4: Build — warning-clean**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean. If `lower_named_fd` is now unused, it is deleted (Step 3), not `#[allow]`ed.

- [ ] **Step 5: Verify NO behavior change from the C branch**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
tests/scripts/fd_torture_diff_check.sh | tail -1
ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | sed -n '2,3p'
```
Expected: lib ok (~1806), fd_torture `32, Pass: 32` **wait — this branch inherits the C branch's fd_torture which is 38** → `Total: 38, Pass: 38, Fail: 0`; named_fd `7 passed`; audit **still 22 DIVERGE** (identical to the C branch — this task is a pure refactor).

- [ ] **Step 6: Commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v292b T1: extract lower_one_redirect shared resolver (#139)

Factor the per-redirect resolution out of lower_redirects + lower_named_fd into
one lower_one_redirect (validate_source routes dup validation to real fds or the
fd_state simulation via an Option). lower_redirects becomes a batch loop over it
(child path). Behavior-preserving: audit unchanged at 22, fd_torture 38/38.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Interleaved in-process applier — the fix

Replace the in-process batch `apply_plan` with an interleaved `apply_redirects` that resolves-then-applies each redirect against the real fds. This makes `{var}` side effects (assign `$v`, alloc fd, persist) visible to the next redirect — fixing the 6 regressions.

**Files:** Modify `crates/huck-engine/src/executor.rs`, `tests/scripts/fd_torture_diff_check.sh`.

**Interfaces:**
- Consumes: `lower_one_redirect` (Task 1), the `RedirectScope` methods `redirect`/`close_target`/`reap_heredoc_writers`/`saved`/`heredoc_writers`.
- Produces: `RedirectScope::apply_redirects` + `RedirectScope::apply_one`. Removes `apply_plan`.

- [ ] **Step 1: Add `apply_one`** (the per-op applier — the arms of the C-branch `apply_plan`, one op at a time, validation REMOVED because `lower_one_redirect(None)` already validated dups against the real fds):
```rust
    fn apply_one(
        &mut self,
        op: PlanOp,
        shell: &mut Shell,
        sink: &mut StdoutSink,
        err_sink: &mut StderrSink,
    ) -> Result<(), ExecOutcome> {
        use std::os::fd::{AsRawFd, IntoRawFd};
        match op {
            PlanOp::InstallOwned { target, source } => {
                let raw = source.as_raw_fd();
                if raw == target {
                    let fd = source.into_raw_fd();
                    unsafe {
                        let flags = libc::fcntl(fd, libc::F_GETFD);
                        if flags >= 0 {
                            let _ = libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                        }
                    }
                    self.saved.push((target, -1));
                } else if self.redirect(shell, raw, target, sink, err_sink).is_err() {
                    return Err(ExecOutcome::Continue(1));
                } else {
                    drop(source);
                }
            }
            PlanOp::InstallDup { target, source } => {
                if self.redirect(shell, source, target, sink, err_sink).is_err() {
                    return Err(ExecOutcome::Continue(1));
                }
            }
            PlanOp::Close { target } => self.close_target(target),
            PlanOp::NamedFd { high, name } => {
                let fd = high.into_raw_fd();
                shell.set(&name, fd.to_string());
            }
        }
        Ok(())
    }
```

- [ ] **Step 2: Add `apply_redirects`** (the interleaved driver):
```rust
    /// Resolve-then-apply each redirection in source order against the shell's
    /// real fds. Interleaved, so a `{var}`'s $v assignment + fd allocation and
    /// each redirect's fd-table mutation are visible to the NEXT redirect —
    /// matching bash and the pre-C `apply`.
    fn apply_redirects(
        &mut self,
        redirs: &[Redirection],
        shell: &mut Shell,
        sink: &mut StdoutSink,
        err_sink: &mut StderrSink,
    ) -> Result<(), ExecOutcome> {
        for redir in redirs {
            let ops = lower_one_redirect(redir, shell, sink, err_sink, None, &mut self.heredoc_writers)
                .map_err(ExecOutcome::Continue)?;
            for op in ops {
                self.apply_one(op, shell, sink, err_sink)?;
            }
        }
        Ok(())
    }
```
Then DELETE `apply_plan`.

- [ ] **Step 3: Rewire the 3 in-process call sites.** Each currently reads:
```rust
    match lower_redirects(redirs, shell, sink, err_sink) {
        Ok(plan) => {
            if let Err(outcome) = scope.apply_plan(plan, shell, sink, err_sink) { <CLEANUP>; return <...>; }
        }
        Err(code) => { <CLEANUP>; return <...>; }
    }
```
Replace each with the interleaved form, preserving that site's exact `<CLEANUP>`/return:
```rust
    if let Err(outcome) = scope.apply_redirects(redirs, shell, sink, err_sink) {
        <CLEANUP>; return <mapped outcome>;
    }
```
- `with_redirect_scope` (~1266) and the builtin-redirect helper (~1385): `<CLEANUP>` = `scope.reap_heredoc_writers(); drop(scope); drain_procsubs(shell, procsub_base);` and `return outcome;` (the `Err(outcome)` from `apply_redirects` is already an `ExecOutcome`).
- `apply_redirects_permanently` (~5204) iterates `&cmd.redirects` and returns `Result<(), ()>`: `if scope.apply_redirects(&cmd.redirects, shell, sink, err_sink).is_err() { scope.reap_heredoc_writers(); return Err(()); }` (drop the `match`/`Ok/Err(code)` arms).

- [ ] **Step 4: Add fd_torture cases pinning the 3 fixed `{var}` regressions** — after the existing Phase-3a parity block in `tests/scripts/fd_torture_diff_check.sh`:
```bash
# --- v292b: in-process {var} interleaving (fixed) ---
check "b nf use later 2>&\$v"  '{ echo err 1>&2; } {v}>f 2>&$v; cat f'
check "b nf persist on fail"   '{ true; } {v}>g 2>&9; echo "v=${v-unset}"'
check "b nf num mixed list"    '{ true; } 3>a {v}>x; echo "v=$v"'
```

- [ ] **Step 5: Build — warning-clean; `apply_plan` gone**
```bash
cargo build -p huck 2>&1 | tail -20
grep -n 'fn apply_plan\|scope.apply_plan\|\.apply_plan(' crates/huck-engine/src/executor.rs || echo "apply_plan GONE"
```
Expected: warning-clean; `apply_plan` has no matches.

- [ ] **Step 6: Verify the fix + the acceptance gate**
```bash
tests/scripts/fd_torture_diff_check.sh | tail -1
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh > /tmp/b_audit.txt 2>&1
sed -n '2,3p' /tmp/b_audit.txt
echo "--- the 6 in-process {var} labels must be ABSENT: ---"
grep -E 'nf\+use \{v\}>f 2>&\$v|nf\+fail \{v\}>f 2>&9|nf\+num 3>a \{v\}>x' /tmp/b_audit.txt || echo "  ABSENT (good)"
```
Expected: fd_torture `Total: 41, Pass: 41, Fail: 0` (38 + 3 new); lib ok; named_fd `7 passed`; audit **`16 DIVERGE`**; the 6 `{var}` labels ABSENT.

- [ ] **Step 7: Full sweep on both binaries**
```bash
cargo build --release -p huck 2>&1 | tail -1
( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )
```
Expected: `... passed, 0 failed`.

- [ ] **Step 8: Commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/fd_torture_diff_check.sh
git commit -m "$(cat <<'EOF'
v292b T2: interleaved in-process redirect apply — fixes {var} regressions (#139)

Replace the batch apply_plan with apply_redirects: resolve-then-apply each
redirect against the real fds, so a {var}'s $v assignment / fd allocation /
persistence is visible to the next redirect (matches bash). Fixes the 6
in-process {var} divergences (2>&$v visibility, persist-on-fail, 3>a {v}>x
numbering); the child batch path is unchanged (8 external fixes retained).
Audit: 22 -> 16 (the persistent orthogonal set). fd_torture 41/41.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the whole-branch review

- **Acceptance is the audit delta, not just green tests.** The C branch was green on the sweep yet had the 6 regressions (the sweep didn't cover invalid-dup-before-file / `{var}` interleaving). Confirm `tools/redirect_audit.sh` = 16 on this branch, none of the 6 `{var}` labels, no new label, and the 8 external fixes still absent from the divergence list.
- **Interleaving correctness:** verify a `{var}` applied before a later failing redirect PERSISTS (not rolled back — it is not in `scope.saved`), that `2>&$v` sees `$v` (because `apply_one`'s `NamedFd` runs `shell.set` before the next `lower_one_redirect`), and that `3>a {v}>x` numbers `$v=10` (the File temp is dropped/closed in `apply_one` before the `{var}`'s `lower_one_redirect` runs `dup_to_high_fd`).
- **Child path unchanged:** `lower_redirects`/`redir_plan_to_child`/`fd_state` behavior identical to the C branch (Task 1 is a pure refactor; Task 2 does not touch the child). named_fd 7/7.
- **Scope:** #137/#140/#141 divergences must remain (do not fix). Slot path + H7 untouched.
