# P4: Merge the pipeline functions into `spawn_pipeline` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse `run_multi_stage` (foreground) and `run_background_sequence` (background) — a near-verbatim ~800-line clone pair — into one shared `spawn_pipeline(commands, mode) -> SpawnedPipeline` core with two thin epilogue wrappers, deleting the duplication without changing any behavior.

**Architecture:** Foreground is a strict superset of background, so `spawn_pipeline` is extracted from `run_multi_stage` with four `mode`-guards disabling the foreground-only bits (capture wiring, live-pid registry, heredoc-writer accumulation, stage-0 inherit) for background. The two public entry points survive as wrappers that call `spawn_pipeline` and run their own epilogue (foreground: capture-drain + `wait_pipeline_raw` + `$PIPESTATUS` + reap; background: register job). The now-dead `build_child_extra_ops` + `spawn_external_with_fds` `None` branch are deleted, the two bail helpers merge, and three residual review nits (#147) are folded in.

**Tech Stack:** Rust (`crates/huck-engine/src/executor.rs`), the huck differential-audit harnesses (`tools/*_audit.sh`, `tests/scripts/*_diff_check.sh`).

## Global Constraints

- **Behavior-preserving.** This iteration fixes NO bug and adds NO behavior. The acceptance gate for every task is that all existing differential checks stay exactly where they are — any movement is a regression. Closes exactly one issue: **#149**.
- **Reference spec:** `docs/superpowers/specs/2026-07-14-spawn-pipeline-merge-design.md`. Do not deviate from it; if the code contradicts the spec, STOP and report.
- **Test discipline (this box OOMs on `cargo test --workspace` — NEVER run that):** build the binary with `cargo build -p huck`; engine lib `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (~1806 pass); integration `( ulimit -v 6000000; cargo test -p huck --test <name> --jobs 1 -- --test-threads 1 )`; sweep `( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh )`.
- **The audits (run each with `HUCK="$(pwd)/target/debug/huck" bash <tool>`):** `tools/bg_pipeline_redirect_audit.sh` = **10/10, 0 DIVERGE**; `tools/pipeline_redirect_audit.sh` = **15/15, 0 DIVERGE**; `tools/redirect_audit.sh` = **157 cases, 16 DIVERGE** (unchanged). fd_torture **44/44**; named_fd **7/7**; bg_pipeline_line_number **1/1**; full sweep **188/0 on debug AND release**.
- **`RedirectSlot` / `slots_for_simple_path` stay** (builtin-stage base). Only `build_child_extra_ops` + the spawner `None` branch are deleted.
- **Commit trailer, verbatim on every commit:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Run `cargo fmt --all` before each commit.
- **`git diff -w` reindent-honesty check:** after the big extraction (Task 2) and the bg conversion (Task 3), run `git diff -w <base>..HEAD -- crates/huck-engine/src/executor.rs` and confirm the moved code is logic-identical — the only non-whitespace delta is the new `mode` scaffolding / deletions, never a changed line in the moved loop body.

## Current-code landmarks (as of branch base; verify by reading, line numbers are approximate)

- `run_background_sequence`: `executor.rs:2845`–`3708` (ends before `bail_teardown_bg` at 3714). Signature `(pipeline: &Pipeline, shell, sink, err_sink, source: &str) -> ExecOutcome`. Dispatched at `:262`.
- `run_multi_stage`: `executor.rs:6574`–`7528` (ends before `wait_pipeline_raw` at 7529). Signature `(commands: &[Command], shell, sink, err_sink) -> ExecOutcome`. Dispatched at `:2738`.
- `bail_teardown_bg`: `3714`–`3729` (`shell, procsub_base, first_pid, spawned_pids, parent_held`; calls `drain_procsubs` + `cleanup_partial_pipeline_raw` + close held → `Continue(1)`).
- `bail_teardown_stage`: `3736`–`3748` (`shell, procsub_base, parent_held`; `drain_procsubs` + close held → `Continue(1)`; NO `cleanup_partial_pipeline_raw`).
- `spawn_external_with_fds`: `8449`; param `plan: Option<ChildRedirPlan>` at `8461`; the `if let Some(p) = plan { … } else { … }` branch at ~`8493`; the dead `else` calls `build_child_extra_ops` at `8527`.
- `build_child_extra_ops`: `5948`.
- `fork_and_run_in_subshell`: `8246` (`cmd, shell, stdio, pgid_target, parent_fds_to_close, stdout_dup_target, stderr_dup_target`).
- `classify_stage`: `8421` → `StageKind::External(&SimpleCommand)` | `StageKind::InProcess(&Command)`.
- `PipelineStage`: `6360` — `enum PipelineStage { Forked(i32) }`.
- Foreground-only landmarks inside `run_multi_stage`: `capture_read_fd` init `6589`; `capture_err_pipe_write_fd`/`capture_err_read_fd` init `6616`; `live_pids_arc = shell.live_external_children.clone()` `6598`; `stage_pids`/`pipeline_stages` `6600`–`6601`; capture stdout arms `6706`/`7146`; capture stderr arm `7214`; per-stage `live_pids_arc.push` + `pipeline_stages.push` `7382`–`7383`; epilogue capture-drain `7396`–`7441`, `wait_pipeline_raw(&pipeline_stages, …)` `7445`+, `$PIPESTATUS` set inside `wait_pipeline_raw`.

---

### Task 1: Delete the dead slot fallback and narrow the spawner (+ nit 3)

**Files:** Modify `crates/huck-engine/src/executor.rs`.

**Interfaces:**
- Consumes: `spawn_external_with_fds(cmd, shell, sink, err_sink, stdio, pgid_target, parent_fds_to_close, plan: Option<ChildRedirPlan>)` (current), `build_child_redir_plan`, `ChildRedirPlan`.
- Produces: `spawn_external_with_fds(…, plan: ChildRedirPlan)` — **non-optional** final param; the two call sites pass the unwrapped plan.

**Why safe:** `build_child_extra_ops` is reachable ONLY from the `spawn_external_with_fds` `else` (`plan == None`) branch. Both pipeline call sites (`run_multi_stage`, `run_background_sequence`) build `external_plan` gated on `stage_is_external` and pass `Some(p)` in the `StageKind::External` arm — the arm only runs for external stages, so `external_plan` is always `Some` there. Single external commands use `run_subprocess`, not this spawner. Therefore the `else` branch never executes.

- [ ] **Step 1: Confirm no caller passes `None`.** Run:
```bash
grep -n 'spawn_external_with_fds(' crates/huck-engine/src/executor.rs
```
Expected: exactly two INVOCATION sites — `StageKind::External(simple) => spawn_external_with_fds(...)` at ~`3602` (bg) and ~`7330` (fg), plus the `fn` definition at `8449`. Read both invocation arms and confirm each passes `external_plan` (the `Option` built above, always `Some` in the External arm).

- [ ] **Step 2: In `spawn_external_with_fds`, replace the `Option` split with the `Some`-body only.** Change the signature param `plan: Option<ChildRedirPlan>` to `plan: ChildRedirPlan`. Replace the whole:
```rust
let stdout_dup_target: Option<i32>;
let stderr_dup_target: Option<i32>;
let replay_ops: Vec<ChildRedirOp>;
let held: Vec<std::os::fd::OwnedFd>;
if let Some(p) = plan {
    // Full-plan path … 
    stdout_dup_target = None;
    stderr_dup_target = None;
    replay_ops = p.ops;
    held = p.held;
    // p.heredoc_writers: …
} else {
    // LEGACY slot path … build_child_extra_ops … 
    …
}
```
with the unconditional plan body:
```rust
// External pipeline stages replay their full ordered ChildRedirPlan; no slot
// dup-targets, no extra_ops. (heredoc_writers: the fg caller drains them; the
// bg caller leaves them in the plan to drop as a no-op — SIGCHLD reaps.)
let stdout_dup_target: Option<i32> = None;
let stderr_dup_target: Option<i32> = None;
let replay_ops: Vec<ChildRedirOp> = plan.ops;
let held: Vec<std::os::fd::OwnedFd> = plan.held;
```
Leave everything downstream (`extra_targets` from `replay_ops`, the `if !replay_ops.is_empty()` pre_exec replay, `drop(held)`, the `fds_to_close` filter) UNCHANGED — it already reads the unified `replay_ops`/`held` names. Since `stdout_dup_target`/`stderr_dup_target` are now always `None`, the dup `pre_exec` block is dead but harmless; leave it (it is guarded by `.is_some()`).

- [ ] **Step 3: Nit 3 — repair the now-stale comment.** In the same region, the comment that describes the deleted `else`/legacy slot path (e.g. "LEGACY slot path: retained but no longer reached …") is gone with the branch; ensure no comment remains claiming a slot fallback exists here. The block comment should describe only the plan replay.

- [ ] **Step 4: Delete `build_child_extra_ops`.** Remove the entire `fn build_child_extra_ops(…)` at `5948`. Also delete its doc-comment references if they name it as a live path (search `build_child_extra_ops`).

- [ ] **Step 5: Update the two call sites to pass the unwrapped plan.** At the bg (`~3602`) and fg (`~7330`) `StageKind::External(simple) => spawn_external_with_fds(…, external_plan)` arms, `external_plan` is currently `Option<ChildRedirPlan>` built above and always `Some` in this arm. Change each to pass the unwrapped value: bind the plan as non-`Option` where it is built for the external arm, OR unwrap at the call with `external_plan.expect("external stage always has a ChildRedirPlan")`. Prefer restructuring the `let external_plan` so that in the External arm it is a plain `ChildRedirPlan` (keep the `Err` bail unchanged); if that is invasive at this stage, `.expect(...)` is acceptable and will be cleaned up when the loop is extracted in Task 2.

- [ ] **Step 6: Build — warning-clean.**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean (no `dead_code` for `build_child_extra_ops` — it is deleted; no unused-variable for the dup targets).

- [ ] **Step 7: Verify NO behavior change.**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/bg_pipeline_redirect_audit.sh | grep '^AUDIT'
tests/scripts/fd_torture_diff_check.sh | tail -1
```
Expected: lib ~1806 ok; single-cmd `16 DIVERGE`; fg pipeline `0 DIVERGE`; bg pipeline `0 DIVERGE`; fd_torture `44`.

- [ ] **Step 8: Commit.**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v295 T1: delete dead build_child_extra_ops + narrow spawner plan to non-optional (#149)

Post-P3b both pipeline callers pass Some(plan) for external stages and single
externals use run_subprocess, so the spawn_external_with_fds None-branch (and
build_child_extra_ops it called) is dead. Delete both; narrow the spawner's
plan param to ChildRedirPlan. Behavior-preserving; audits + fd_torture unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Extract `spawn_pipeline` from `run_multi_stage`; make foreground a thin wrapper

**Files:** Modify `crates/huck-engine/src/executor.rs`.

**Interfaces:**
- Consumes: `spawn_external_with_fds(…, plan: ChildRedirPlan)` (Task 1), `fork_and_run_in_subshell`, `classify_stage`, `build_child_redir_plan`, `async_default_stdin`, `make_pipe`, `make_orphan_pipe_for_eof_reader`, `cleanup_partial_pipeline_raw`, `drain_procsubs`, `PipelineStage::Forked`, `ChildFd`.
- Produces:
```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpawnMode { Foreground, Background }

struct SpawnedPipeline {
    first_pid: Option<i32>,
    stages: Vec<PipelineStage>,
    heredoc_writers: Vec<i32>,
    pgid_target: i32,
    procsub_base: usize,
    capture_read_fd: Option<RawFd>,
    capture_err_read_fd: Option<RawFd>,
}

fn spawn_pipeline(
    commands: &[Command],
    mode: SpawnMode,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<SpawnedPipeline, ExecOutcome>;

fn bail_teardown_pipeline(
    mode: SpawnMode,
    shell: &mut Shell,
    procsub_base: usize,
    first_pid: Option<i32>,
    stages: &[PipelineStage],
    parent_held: &mut Vec<RawFd>,
) -> ExecOutcome;
```
  After this task, `run_multi_stage` is a thin wrapper; `run_background_sequence` is UNCHANGED (still its own loop, still uses `bail_teardown_bg`). `spawn_pipeline` is called only by the foreground wrapper this task; Task 3 points background at it.

- [ ] **Step 1: Add the `SpawnMode` enum and `SpawnedPipeline` struct** near the other pipeline types (by `PipelineStage`, `~6360`). Exact code as in the Interfaces block above. `RawFd` is already imported in this file.

- [ ] **Step 2: Add the merged bail helper `bail_teardown_pipeline`** next to the existing bail helpers (`~3736`). Exact body:
```rust
/// Unified pipeline-spawn error teardown. Both modes drain process substitutions
/// started this pipeline and close every parent-held pipe fd, then return
/// failure. Background additionally kills+reaps the already-spawned stages;
/// foreground does NOT — it tracks live pids via live_external_children/`stages`
/// and reaps them through wait_pipeline_raw, so reaping here would double-reap.
fn bail_teardown_pipeline(
    mode: SpawnMode,
    shell: &mut Shell,
    procsub_base: usize,
    first_pid: Option<i32>,
    stages: &[PipelineStage],
    parent_held: &mut Vec<RawFd>,
) -> ExecOutcome {
    drain_procsubs(shell, procsub_base);
    if mode == SpawnMode::Background {
        let pids: Vec<i32> = stages.iter().map(|PipelineStage::Forked(p)| *p).collect();
        cleanup_partial_pipeline_raw(first_pid, &pids);
    }
    for fd in parent_held.drain(..) {
        unsafe {
            libc::close(fd);
        }
    }
    ExecOutcome::Continue(1)
}
```

- [ ] **Step 3: Create `fn spawn_pipeline` by MOVING `run_multi_stage`'s loop body into it.** `spawn_pipeline` owns everything from the pre-loop setup (`interactive`/`n`/`first_pid`/`stage_pids`→`stages`/`live_pids_arc`/`capture_*` init + `procsub_base = shell.procsub_pending.len()`) through the end of the stage `for` loop, but NOT the epilogue (capture-drain, `wait_pipeline_raw`, `$PIPESTATUS`, `live_external_children` clear). It returns `Ok(SpawnedPipeline { first_pid, stages, heredoc_writers, pgid_target, procsub_base, capture_read_fd, capture_err_read_fd })`. Rename the foreground pid vec to `stages` (it was `pipeline_stages`); keep pushing `PipelineStage::Forked(pid)`. Every current `return bail_teardown_stage(shell, procsub_base, &mut parent_held);` inside the moved loop becomes `return Err(bail_teardown_pipeline(mode, shell, procsub_base, first_pid, &stages, &mut parent_held));`. Every other early `return ExecOutcome::…` for a spawn failure inside the loop becomes `return Err(that outcome)`. The heredoc-writer accumulator (`heredoc_writers`) is declared in `spawn_pipeline` and returned.

- [ ] **Step 4: Wrap the four `mode` guards into the moved loop** (spec §"The four in-loop divergences"), each preserving the foreground behavior for `Foreground` and giving background its simpler behavior for `Background`:
  1. **Stage-0 stdin default.** Where foreground computes the first stage's stdin default (currently `ChildFd::Inherit`/`STDIN_FILENO`), compute a pre-loop `let stage0_default: ChildFd = match mode { SpawnMode::Foreground => ChildFd::Inherit, SpawnMode::Background => async_default_stdin(inherit_flag, shell, sink, err_sink).map_err(|()| ExecOutcome::Continue(1))? };` — mirror `run_background_sequence`'s existing `async_default_stdin` call for the `inherit_flag` value. Use `stage0_default.try_clone()` for stage 0 exactly as background does today (foreground's inherit is equivalent).
  2. **Heredoc-writer disposition.** At each site that obtains a forked heredoc/here-string writer pid (external via `plan.heredoc_writers`; builtin-InProcess via `spawn_heredoc_writer`), `match mode { SpawnMode::Foreground => heredoc_writers.push(pid), SpawnMode::Background => { /* drop: SIGCHLD reaps */ } }`.
  3. **Live-pid registry.** Where foreground does `live_pids_arc.lock().unwrap().push(pid)` (`~7382`), guard it `if mode == SpawnMode::Foreground { live_pids_arc.lock().unwrap().push(pid as libc::pid_t); }`. `stages.push(PipelineStage::Forked(pid))` stays unconditional.
  4. **Last-stage stdout/stderr construction.** The last-stage stdout `else` block and the stderr assignment become `match mode`: `Foreground` keeps the existing sink-typed arms verbatim (`StdoutSink::Capture(_) => make_pipe`+record `capture_read_fd`; `StderrSink::Capture(_) => dup` the shared `capture_err` write end; else `Inherit`); `Background` is `ChildFd::Inherit` for both. The pre-loop `capture_err_pipe_write_fd` setup runs only in `Foreground` (it already keys on `StderrSink::Capture`, which background never carries — but gate it on `mode == Foreground` too, so background never allocates it).

- [ ] **Step 5: Rewrite `run_multi_stage` as the foreground wrapper.** Body:
```rust
fn run_multi_stage(
    commands: &[Command],
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let sp = match spawn_pipeline(commands, SpawnMode::Foreground, shell, sink, err_sink) {
        Ok(sp) => sp,
        Err(outcome) => return outcome,
    };
    // The rest is the EXISTING foreground epilogue, now reading from `sp`:
    //  - if interactive && let Some(pgid) = sp.first_pid { give_terminal_to(pgid); }
    //  - drain sp.capture_read_fd / sp.capture_err_read_fd via stream_loop::CaptureSinks,
    //    folding captured stderr into err_sink (verbatim move of the current 7396-7441 block,
    //    with capture_read_fd/capture_err_read_fd sourced from `sp`);
    //  - last_status = wait_pipeline_raw(&sp.stages, …) (sets $PIPESTATUS);
    //  - reap sp.heredoc_writers (the current heredoc-writer reap loop);
    //  - bulk-clear shell.live_external_children (the current post-wait clear);
    //  - drain_procsubs(shell, sp.procsub_base);
    //  - return ExecOutcome::Continue(last_status) exactly as today.
}
```
Move the current epilogue (from after the stage loop to the end of `run_multi_stage`) verbatim into this wrapper, replacing the local `capture_read_fd`/`capture_err_read_fd`/`pipeline_stages`/`procsub_base`/`heredoc_writers` reads with the `sp.*` fields. Keep the `$PIPESTATUS` set inside `wait_pipeline_raw` — `run_multi_stage` remains the leaf-site.

- [ ] **Step 6: Build — warning-clean.**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean. `bail_teardown_stage` is now unused (foreground uses `bail_teardown_pipeline`) → delete it in this task (background still uses `bail_teardown_bg`, untouched). If the compiler flags `SpawnMode`/`SpawnedPipeline` fields unused, it is because a background-only field (`heredoc_writers` empty, `capture_*` None) is not yet read on the fg path — that is expected; the fields are read by the fg wrapper and (Task 3) the bg wrapper, so no `dead_code` should fire once the wrapper reads them all.

- [ ] **Step 7: Verify foreground is byte-identical + nothing else moved.**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/bg_pipeline_redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'
tests/scripts/fd_torture_diff_check.sh | tail -1
( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3 )
git diff -w HEAD~1..HEAD -- crates/huck-engine/src/executor.rs | grep -E '^[+-]' | grep -vE '^[+-]{3}' | grep -viE 'spawn_pipeline|SpawnMode|SpawnedPipeline|bail_teardown_pipeline|bail_teardown_stage|mode|sp\.|match |=> |Foreground|Background|fn run_multi_stage|heredoc_writers|capture_read_fd|capture_err_read_fd|stages|Ok\(sp\)|Err\(outcome\)|return outcome|^\+\s*//|^\-\s*//' | head -40
```
Expected: lib ~1806 ok; fg pipeline `0 DIVERGE`; bg pipeline `0 DIVERGE` (background untouched); single-cmd `16 DIVERGE`; fd_torture `44`; named_fd 7/7. The final `git diff -w` filter surfaces moved loop-body lines that changed for reasons OTHER than the extraction scaffolding — it should print (near-)nothing; investigate any real code line it surfaces.

- [ ] **Step 8: Commit.**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v295 T2: extract spawn_pipeline from run_multi_stage; foreground becomes a wrapper (#149)

Move run_multi_stage's stage loop into spawn_pipeline(commands, mode) ->
SpawnedPipeline, with four mode-guards disabling the foreground-only bits
(capture wiring, live-pid registry, heredoc accumulation, stage-0 inherit) for
Background. run_multi_stage is now a thin foreground wrapper (capture-drain +
wait_pipeline_raw + $PIPESTATUS + reap). Merge the fg bail helper into
bail_teardown_pipeline; delete bail_teardown_stage. run_background_sequence
unchanged (Task 3 points it at spawn_pipeline). Behavior-preserving; audits +
sweep unchanged; git diff -w confirms the moved loop is logic-identical.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Convert `run_background_sequence` to a wrapper; delete the clone

**Files:** Modify `crates/huck-engine/src/executor.rs`.

**Interfaces:**
- Consumes: `spawn_pipeline(commands, SpawnMode::Background, …)` and `SpawnedPipeline` (Task 2), `bail_teardown_pipeline` (Task 2), `jobs.add_with_pgroup`, `jobs.add_synthetic_done`, `reap_and_notify`, `display_command`, `setpgid_self`.
- Produces: `run_background_sequence` reduced to a thin background wrapper; the ~800-line clone loop and `bail_teardown_bg` deleted.

- [ ] **Step 1: Read `run_background_sequence` and confirm its loop matches the `Background` arms of `spawn_pipeline`.** Specifically verify (by reading) that `spawn_pipeline` with `mode == Background` reproduces: stage-0 stdin via `async_default_stdin`, heredoc writers dropped, no `live_external_children` push, last-stage stdout/stderr `Inherit`, and the `bail_teardown_pipeline(Background, …)` teardown. If any background-specific step is NOT reproduced by `spawn_pipeline`, STOP and report — do not paper over it in the wrapper.

- [ ] **Step 1b (divergence #5 — the Task-2 review found this bg-arm gap; MUST fix before the flip): mode-key the `first_pid` and pgid computation.** In `spawn_pipeline`, foreground was extracted as: `first_pid` set only `if interactive && first_pid.is_none()`, and every `pgid_target`/leader computation keyed on `interactive` (`= job_control_active() && sink==Terminal`). Background instead sets `first_pid` UNCONDITIONALLY on the first stage (it is the job's leader pid, needed even with job control off) and keys pgrouping on `job_control` alone. Fix in `spawn_pipeline`:
  - The `first_pid` assignment (both spawn sites) → `if (mode == SpawnMode::Background || interactive) && first_pid.is_none() { first_pid = Some(pid); }`.
  - Introduce a mode-aware pgrouping predicate: `let group = match mode { SpawnMode::Foreground => interactive, SpawnMode::Background => job_control };` (compute `job_control = shell.job_control_active()` once, pre-loop) and use `group` wherever `pgid_target` was computed as `if interactive { first_pid.unwrap_or(0) } else { NO_PGROUP }` → `if group { first_pid.unwrap_or(0) } else { NO_PGROUP }`.
  This is behavior-neutral for foreground (`group == interactive` there) and gives background the leader pid + `job_control`-keyed pgrouping its wrapper needs (`add_with_pgroup`, `$!`). Without it a non-interactive `a | b &` returns `first_pid: None` → the wrapper wrongly takes the `add_synthetic_done` path (no job registered).

- [ ] **Step 1c (also from the Task-2 review): route the two non-bail error sites through `bail_teardown_pipeline`.** In `spawn_pipeline` there are two spawn-failure sites that were extracted with inline foreground teardown (`drain_procsubs` + close `parent_held` + `return Err(ExecOutcome::Continue(1))`) and NO `cleanup_partial_pipeline_raw`: the orphan-pipe (M-125) creation failure on a non-final stage with an explicit stdout redirect, and the general stage-spawn failure. Foreground is correct as-is (it reaps via its own waiter), but Background must kill+reap already-spawned stages there. Replace each inline body with `return Err(bail_teardown_pipeline(mode, shell, procsub_base, first_pid, &stages, &mut parent_held));` — the `Foreground` arm is byte-equivalent to the current inline body, and the `Background` arm adds the needed `cleanup_partial_pipeline_raw`. (Search `spawn_pipeline` for `drain_procsubs` calls that are NOT already `bail_teardown_pipeline`.)

- [ ] **Step 2: Replace the body of `run_background_sequence` with the background wrapper.** Body:
```rust
fn run_background_sequence(
    pipeline: &Pipeline,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
    source: &str,
) -> ExecOutcome {
    let display = display_command(source);
    let job_control = shell.job_control_active();
    let sp = match spawn_pipeline(&pipeline.commands, SpawnMode::Background, shell, sink, err_sink) {
        Ok(sp) => sp,
        Err(outcome) => return outcome,
    };
    // Existing background epilogue, now reading from `sp`:
    //  - setpgid each pid in sp.stages to the leader (setpgid_self / the current
    //    per-pid setpgid loop);
    //  - let Some(pgid) = sp.first_pid else { shell.jobs.add_synthetic_done(display, 0);
    //        crate::jobs::reap_and_notify(shell); return ExecOutcome::Continue(0); };
    //  - let pids: Vec<i32> = sp.stages.iter().map(|PipelineStage::Forked(p)| *p).collect();
    //  - shell.jobs.add_with_pgroup(pgid, pids, display, job_control);
    //  - return ExecOutcome::Continue(0).
}
```
Move the current background epilogue (job registration, synthetic-done, `reap_and_notify`) verbatim, sourcing pids/leader from `sp`. `sp.heredoc_writers` is empty and `sp.capture_*` are `None` in background mode — do not read them.

- [ ] **Step 3: Delete `bail_teardown_bg`** (`3714`–`3729`) — now unused (background bails via `bail_teardown_pipeline` inside `spawn_pipeline`).

- [ ] **Step 4: Build — warning-clean.**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean; `bail_teardown_bg` gone with no remaining caller; no `dead_code` on any `SpawnedPipeline` field (both wrappers now collectively read all fields).

- [ ] **Step 5: Verify background is byte-identical + the deletion is clean.**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
HUCK="$(pwd)/target/debug/huck" bash tools/bg_pipeline_redirect_audit.sh
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'
tests/scripts/fd_torture_diff_check.sh | tail -1
( ulimit -v 6000000; cargo test -p huck --test bg_pipeline_line_number_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3 )
( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -3 )
```
Expected: lib ~1806 ok; bg pipeline `AUDIT: 10 cases, 10 agree, 0 DIVERGE`; fg pipeline `0 DIVERGE`; single-cmd `16 DIVERGE`; fd_torture `44`; bg_pipeline_line_number 1/1; named_fd 7/7. If any bg audit case diverges, STOP and report its bash-vs-huck result-file diff — do not edit the audit.

- [ ] **Step 6: Full sweep on both binaries.**
```bash
cargo build --release --locked -p huck 2>&1 | tail -1
( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )
```
Expected: `Diff-check sweep: 188 passed, 0 failed`.

- [ ] **Step 7: Commit.**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v295 T3: run_background_sequence becomes a spawn_pipeline wrapper; delete the clone (#149)

Reduce run_background_sequence to a thin background wrapper over
spawn_pipeline(Background) + its job-register epilogue, deleting the ~800-line
hand-maintained clone of the stage loop and the now-unused bail_teardown_bg.
Behavior-preserving; bg audit 10/10, fg + single-cmd unchanged, sweep 188/0
both binaries.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Fold in nits 1 & 2 (double `classify_stage`, dup-target guard)

**Files:** Modify `crates/huck-engine/src/executor.rs` (`spawn_pipeline`).

**Interfaces:**
- Consumes: `spawn_pipeline` (Tasks 2–3), `classify_stage`, `StageKind`.
- Produces: no signature change; internal cleanup only.

- [ ] **Step 1: Nit 1 — classify once.** In `spawn_pipeline`'s loop, `classify_stage(stage_cmd, shell)` is called both for the hoisted `stage_is_external` and again at the spawn `match`. Bind it once: `let kind = classify_stage(stage_cmd, shell);` near the top of the iteration, derive `let stage_is_external = matches!(kind, StageKind::External(_));`, and `match kind { StageKind::External(simple) => …, StageKind::InProcess(cmd) => … }` at the spawn site. Borrow note: `kind` borrows `stage_cmd` (a `&Command`) immutably; the intervening `build_child_redir_plan`/base construction borrows `shell` mutably but not `stage_cmd`, so one `kind` binding held across the iteration is fine — if the borrow checker objects, keep `stage_is_external: bool` and re-`match classify_stage(...)` ONLY at the spawn site (this still removes the redundant *middle* call), and note it in the report.

- [ ] **Step 2: Nit 2 — guard the dup-target block.** The `sdt`/`sedt` (`slot_stdout()`/`slot_stderr()` Dup) resolution block that feeds `fork_and_run_in_subshell`'s `stdout_dup_target`/`stderr_dup_target` currently runs for all stages; external stages get dup handling from the replayed plan and ignore these. Guard the block `if !stage_is_external { … } else { (None, None) }` (or equivalent) so the dup-target words are expanded only for InProcess stages — removing the wasteful double-expansion of any side-effecting dup-target word on external stages.

- [ ] **Step 3: Build — warning-clean.**
```bash
cargo build -p huck 2>&1 | tail -20
```
Expected: warning-clean.

- [ ] **Step 4: Verify unchanged behavior.**
```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
HUCK="$(pwd)/target/debug/huck" bash tools/bg_pipeline_redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'
tests/scripts/fd_torture_diff_check.sh | tail -1
```
Expected: lib ~1806 ok; bg `0 DIVERGE`; fg `0 DIVERGE`; single-cmd `16 DIVERGE`; fd_torture `44`.

- [ ] **Step 5: Full sweep on both binaries.**
```bash
( ulimit -v 1500000; timeout 1200 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )
```
Expected: `Diff-check sweep: 188 passed, 0 failed` (both binaries already built).

- [ ] **Step 6: Commit.**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v295 T4: fold in the #147 nits — classify once, guard dup-target expansion (#149)

In the unified spawn_pipeline loop: bind classify_stage once (no second call at
the spawn match) and guard the slot dup-target resolution block !stage_is_external
so external stages no longer double-expand their dup-target words. No behavior
change; audits + sweep unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the whole-branch review

- **Acceptance is the audits + sweep, unchanged:** bg `10/10`, fg `15/15`, single-cmd `16`, fd_torture `44`, named_fd `7/7`, bg_pipeline_line_number `1/1`, sweep `188/0` on both binaries. Any movement is a regression, not an expected change.
- **The two correctness-critical `mode` branches:** heredoc-writer disposition (fg accumulate+reap / bg drop) and last-stage capture-vs-inherit (bg must stay `Inherit`, never spawn a capture pipe — protects `$(a | b &)`). Confirm both are `mode`-gated, not sink-gated, in `spawn_pipeline`.
- **Scope containment:** the diff touches only `executor.rs`. `RedirectSlot`/`slots_for_simple_path` retained; `run_multi_stage`/`run_background_sequence` names + dispatch sites (`:262`, `:2738`) + the `$PIPESTATUS` leaf-site invariant preserved. #144/#145/#142/#141/#140/#137/#78/#79 untouched; the PR closes only #149 and narrows #147.
- **`git diff -w` on the full branch:** the moved loop body must be logic-identical; the only non-whitespace deltas are the `mode` scaffolding, the deletions, and the two nit fixes.
