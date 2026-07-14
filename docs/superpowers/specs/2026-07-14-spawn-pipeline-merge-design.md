# Merge the two pipeline functions into `spawn_pipeline` (P4) — closes #149

**Issue:** [#149](https://github.com/jdstanhope/huck/issues/149) — merge `run_multi_stage` + `run_background_sequence` into one `spawn_pipeline` core. **Phase:** fd-plumbing remediation **P4** (review: `docs/superpowers/reviews/2026-07-13-engine-fd-plumbing-review.md` §Phase 4). **Builds on:** P3a/P3b (v292–v294) which flipped both pipeline paths onto the ordered `ChildRedirPlan` replay, leaving the two functions differing only in epilogue + five small in-loop policy points.

## Why (one paragraph)

`run_background_sequence` (~870 lines) is a near-verbatim, hand-maintained clone of `run_multi_stage` (~1,050 lines) — the review's §2.2(a) names seven near-identical block pairs, and its own header admits it ("Spawn each stage using the same per-stage fork dispatch as run_multi_stage"). Every per-feature fix (`RedirOp::Move`, async-stdin #126/#129, CLOEXEC #132) has had to land twice, and each such fix has historically spawned a sibling bug in the un-touched path. After P3b the two bodies differ only in their epilogue (foreground blocks, sets `$PIPESTATUS`, and drains capture sinks; background registers a job and returns) and five in-loop policy points (stage-0 stdin default, heredoc-writer disposition, foreground-only live-pid registration, last-stage capture-vs-inherit wiring, and leader-pid/pgroup keying). This iteration extracts the shared stage loop into one `spawn_pipeline` core with two thin mode-specific epilogue wrappers, deleting the duplication and making those policies single-site permanently. It is a **behavior-preserving dedup**: no bug is fixed or introduced (the review's #126/#129 stage-0 policy is already unified and closed).

## Scope

**In scope (v295):**
- Extract the unified stage-spawning loop into a private `spawn_pipeline(commands, mode, shell, sink, err_sink) -> Result<SpawnedPipeline, ExecOutcome>`.
- Keep `run_multi_stage` and `run_background_sequence` as thin epilogue wrappers over it (dispatch sites `:262`, `:2738` unchanged; `run_multi_stage` stays the `$PIPESTATUS` leaf-site).
- Merge the two bail helpers (`bail_teardown_stage`, `bail_teardown_bg`) into one `bail_teardown_pipeline(mode, …)`.
- Delete `build_child_extra_ops` and the `spawn_external_with_fds` `else` (`plan == None`) branch (both provably dead post-P3b); narrow the spawner's `plan: Option<ChildRedirPlan>` to non-optional `plan: ChildRedirPlan`.
- Fold in the three residual review nits from #147: reuse the hoisted classification at the spawn `match` (no second `classify_stage`); guard the dup-target block `&& !stage_is_external` (InProcess-only); remove the stale spawner comment.

**Explicitly NOT in scope:**
- `RedirectSlot` / `slots_for_simple_path` — retained (still build the **builtin**-InProcess stage base via the last-wins slot path, now written once). Converting builtin-stage base off slots entangles with the H7 software-sink layer and #144, and is deferred; **#147** is narrowed to that remaining conversion.
- Any behavior change. This iteration is a pure dedup, so it fixes **no** behavioral divergence. In particular the adjacent open redirect/pipeline bugs stay exactly as-is, neither closed nor regressed: **#144** (builtin-stage stderr → sink layer), **#145** (pipeline aborts on a stage redirect-setup failure), **#142** (in-process reap-before-restore hang on a >64KB heredoc + failing redirect), **#141** (external `{var}` fd number under child batch lowering), **#140** (`{var}` redirect error wording/`$v`-visibility), **#137** (builtin write-to-closed-stdout swallowed), and **#78**/**#79**. These live in redirect *lowering*, the *in-process* apply path, or the *software-sink* layer — none of which the orchestration merge touches. Procsub/job-control alignment is Phase 5.
- Touching the leaf-level shared helpers (`spawn_external_with_fds` internals beyond the `else`-deletion, `fork_and_run_in_subshell`, `build_child_redir_plan`, `classify_stage`, `wait_pipeline_raw`, `async_default_stdin`, `make_pipe`).

**Issue outcome:** the PR **closes exactly one issue — #149**. It comments on and narrows **#147**. It references none of the divergence bugs above (they are out of scope by construction).

## Design

### The core: `spawn_pipeline`

```rust
enum SpawnMode { Foreground, Background }

struct SpawnedPipeline {
    first_pid: Option<i32>,          // pgroup leader; None => nothing spawned (empty/synthetic-done)
    stages: Vec<PipelineStage>,      // ordered stage pids (PipelineStage::Forked(pid)); fg -> wait_pipeline_raw,
                                     //   bg -> mapped to a pid list for jobs.add_with_pgroup
    heredoc_writers: Vec<i32>,       // Foreground: reaped by the wrapper after wait; Background: empty
    pgid_target: i32,                // the job's process-group target (NO_PGROUP when job control off)
    procsub_base: usize,             // index into shell.procsub_pending; drain_procsubs(shell, procsub_base)
    capture_read_fd: Option<RawFd>,  // Foreground capture-sink only: last stage's stdout pipe, drained by
                                     //   the wrapper. Always None for Background (never captures).
    capture_err_read_fd: Option<RawFd>, // Foreground StderrSink::Capture only: shared stderr pipe read end.
}

fn spawn_pipeline(
    commands: &[Command],
    mode: SpawnMode,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<SpawnedPipeline, ExecOutcome>
```

`spawn_pipeline` owns the entire per-stage loop that is today duplicated: inline-assignment snapshot/restore per stage, `classify_stage`, the external path (build `ChildRedirPlan` before the inter-stage pipe → `spawn_external_with_fds(…, plan)`), the InProcess path (slot base + dup targets → `fork_and_run_in_subshell`), inter-stage `make_pipe` wiring, `parent_held` accumulation and the per-stage close bookkeeping, and pgroup assignment (first stage becomes leader; subsequent stages `setpgid` to it). On any stage failure it calls `bail_teardown_pipeline(mode, …)` and returns `Err(outcome)`. On success it returns the populated `SpawnedPipeline`; the parent-held inter-stage pipe fds are closed as each stage consumes them (unchanged from today), so nothing pipe-related outlives the loop.

`procsub_base` is the pre-loop marker into `shell.procsub_pending` (a `usize` length, today set as `let procsub_base = shell.procsub_pending.len();` before spawning); `spawn_pipeline` captures it and returns it so the wrapper's epilogue drains process substitutions at the right time per mode.

### Extraction strategy: `run_multi_stage` is the superset

Foreground is a strict superset of background: it has everything background does **plus** capture-sink wiring, the timeout-SIGTERM live-pid registry, and heredoc-writer accumulation. So `spawn_pipeline` is extracted from `run_multi_stage`'s loop, with the foreground-only bits guarded off for `Background` via `mode`. Background's simpler behavior (inherit the last stage's stdout, drop heredoc writers, no live-pid registry) falls out of taking the `Background` arm at each guard. The five guards below are the entire `mode`-dependent surface inside the loop; everything else — pipe creation, fd relocation, `parent_held` handling, pgroup bookkeeping, `stages.push(PipelineStage::Forked(pid))`, the external-vs-InProcess spawn dispatch, `make_orphan_pipe_for_eof_reader` for a non-final stage with an explicit stdout redirect — is byte-identical between the two functions today and becomes single-source.

### The five in-loop divergences (all that `mode` decides inside the loop)

1. **Stage-0 stdin default.** `match mode { Foreground => ChildFd::Inherit /* STDIN_FILENO */, Background => async_default_stdin(inherit, shell, sink, err_sink)? }`, computed once before the loop as `stage0_default: ChildFd` and cloned per use exactly as both functions do today. `async_default_stdin` already gates `/dev/null` on `!interactive` and honors the bare-multi-stage inherit rule (#129, closed) — so this is a literal move of existing logic, not a policy change.
2. **Heredoc-writer disposition.** When a stage forks a heredoc/here-string writer (external stages via `build_child_redir_plan`'s `heredoc_writers`; builtin-InProcess stages via `spawn_heredoc_writer` on the `slot_stdin` path), `match mode { Foreground => acc.push(pid), Background => drop }`. Background children are reaped by the SIGCHLD reaper, so their writer pids are intentionally discarded (the v294 contract); foreground reaps them in the wrapper after the pipeline wait.
3. **Foreground-only live-pid registration.** After each stage spawns, the foreground path pushes the pid into `shell.live_external_children` (the `live_pids_arc` mutex) so the timeout timer thread can `SIGTERM` all live stages in one pass when a deadline fires; the background path does not (its stages become a registered job). `if let SpawnMode::Foreground = mode { live_pids_arc.lock().unwrap().push(pid); }`. The matching post-wait bulk *clear* of `live_external_children` stays in the foreground wrapper's epilogue.
4. **Last-stage stdout/stderr construction (capture wiring).** For the final stage with no explicit redirect, `match mode`: `Foreground` runs the existing sink-typed arms — `StdoutSink::Capture(_) => make_pipe` (record `capture_read_fd`), `StderrSink::Capture(_) => dup` the shared capture-err write end, else `ChildFd::Inherit`; `Background` is unconditionally `ChildFd::Inherit` for both (a backgrounded job never captures — a `pipeline &` inside `$(…)` writes to the terminal, matching bash). The pre-loop shared stderr-capture pipe setup (`capture_err_pipe_write_fd`) and the post-loop capture-drain (`stream_loop::CaptureSinks`) are foreground-only and live in the foreground wrapper; `spawn_pipeline` returns `capture_read_fd`/`capture_err_read_fd` in `SpawnedPipeline` for the wrapper to drain. This guard is what makes the merge behavior-preserving for `$(a | b &)`: background keeps `Inherit`, never sprouting a capture pipe.
5. **Leader-pid + pgroup keying.** (Surfaced by the Task-2 review of the extraction.) Foreground tracks a pgroup leader (`first_pid`) and pgroups only when `interactive` (`job_control_active() && sink == Terminal`); background sets `first_pid` **unconditionally** (it is the job's leader pid, needed even with job control off — the wrapper registers it via `add_with_pgroup` and publishes `$!`) and keys pgrouping on `job_control` alone. So `first_pid` assignment is `(mode == Background || interactive)`-gated, and the `pgid_target` predicate is `match mode { Foreground => interactive, Background => job_control }`. Behavior-neutral for foreground (`group == interactive`); required for background — without it a non-interactive `a | b &` would return `first_pid: None` and skip job registration. The two non-bail spawn-failure error sites likewise route through `bail_teardown_pipeline(mode, …)` so background reaps partial pipelines there (foreground behavior unchanged, since its arm omits `cleanup_partial_pipeline_raw`).

### The two epilogue wrappers

```rust
fn run_multi_stage(commands, shell, sink, err_sink) -> ExecOutcome {   // foreground
    let sp = match spawn_pipeline(commands, SpawnMode::Foreground, shell, sink, err_sink) {
        Ok(sp) => sp, Err(outcome) => return outcome,
    };
    // give_terminal_to(sp.first_pid) when interactive; drain sp.capture_read_fd /
    // sp.capture_err_read_fd via stream_loop::CaptureSinks (folding captured stderr
    // into err_sink); wait_pipeline_raw(&sp.stages, …) sets $PIPESTATUS and returns
    // last_status; reap sp.heredoc_writers; clear live_external_children;
    // drain_procsubs(sp.procsub_base); Continue(last_status).
}

fn run_background_sequence(pipeline, shell, sink, err_sink, source) -> ExecOutcome {  // background
    let sp = match spawn_pipeline(&pipeline.commands, SpawnMode::Background, shell, sink, err_sink) {
        Ok(sp) => sp, Err(outcome) => return outcome,
    };
    // setpgid each pid to the leader; if sp.first_pid is None -> add_synthetic_done +
    // reap_and_notify + Continue(0); else map sp.stages -> pids and
    // jobs.add_with_pgroup(pgid, pids, display, job_control); Continue(0).
}
```

Both wrappers keep their current signatures and return types verbatim, so the dispatch layer and `$PIPESTATUS` leaf-site invariant are untouched. The wrapper bodies are the parts that were genuinely *not* duplicated; keeping them separate leaves `spawn_pipeline` with the single responsibility of spawning the stages.

### Deletions and the spawner-signature narrowing

- **`build_child_extra_ops`** is called only from the `spawn_external_with_fds` `else` (`plan == None`) branch. Both pipeline call sites pass `Some(plan)` for external stages (the External spawn arm always has a plan), and single external commands use `run_subprocess`, not `spawn_external_with_fds`. So the `else` branch and `build_child_extra_ops` are dead. Delete both; change the spawner parameter `plan: Option<ChildRedirPlan>` to `plan: ChildRedirPlan`, dropping the `if let Some(p) = plan { … } else { … }` split down to the former `Some` body. The `slot_stdout()/slot_stderr()` dup-target resolution that lived only in that `else` goes with it.
- **Bail helpers:** both `drain_procsubs(procsub_base)` and close every parent-held pipe fd, then return `ExecOutcome::Continue(1)`. They differ in exactly one step: `bail_teardown_bg` also calls `cleanup_partial_pipeline_raw(first_pid, spawned_pids)` to kill+reap the already-spawned stages, whereas `bail_teardown_stage` does **not** — the foreground path tracks live pids via `live_external_children`/`stages` and reaps them through its own waiter, so reaping in the bail helper would double-reap. Merge into one `bail_teardown_pipeline(mode, shell, procsub_base, first_pid, stages, parent_held)` where `mode == Background` performs the `cleanup_partial_pipeline_raw` step and `Foreground` skips it. `mode` is genuinely used here.
- **Nit 1:** at the spawn `match`, reuse the `StageKind` already computed for `stage_is_external` instead of calling `classify_stage` a second time — bind `let kind = classify_stage(...)` once, derive `stage_is_external` from it, and match `kind` at the spawn site.
- **Nit 2:** guard the `sdt`/`sedt` dup-target resolution block `&& !stage_is_external` so it runs only for InProcess stages (external stages get dup handling from the replayed plan; today the block expands the dup-target words for external stages too, wastefully and with a double-expansion of any side-effecting word).
- **Nit 3:** remove/repair the stale `spawn_external_with_fds` comment that references the now-deleted slot dup path.

### `RedirectSlot` stays (why)

`RedirectSlot` / `slots_for_simple_path` are still read by the InProcess `slot_stdin/stdout/stderr` blocks to build the base stdio for **builtin** pipeline stages (a `Simple(Exec)` that resolves to a builtin, not an external and not a compound). Compound InProcess stages skip slots entirely (pipe-only base; the forked body applies its own redirects via `RedirectScope`). Retiring `RedirectSlot` therefore requires reworking builtin-stage base construction onto the ordered lowering, which touches the H7 software-sink layer and #144 — out of scope here, tracked by the narrowed #147.

## Testing

This is a behavior-preserving refactor, so the gate is that **every existing differential check stays exactly where it is** — any movement is a regression, not an expected change:

1. `tools/bg_pipeline_redirect_audit.sh` **10/10 agree, 0 DIVERGE**; `tools/pipeline_redirect_audit.sh` (foreground) **15/15, 0 DIVERGE**; `tools/redirect_audit.sh` (single-command) **16 DIVERGE** (unchanged).
2. `fd_torture_diff_check.sh` **44/44**; engine lib green (~1806); `named_fd_integration` **7/7**; `bg_pipeline_line_number_integration` **1/1**.
3. Full `run_diff_checks.sh` sweep **188 passed, 0 failed** on BOTH the debug and release binaries.
4. Warning-clean build (`cargo build -p huck`), and a `git diff -w` sanity pass so the large mechanical move is verifiably logic-preserving.

No new test or audit is added: there is no new behavior to pin. The existing pipeline-stage differential audits (built in v293/v294 for exactly this surface) plus the sweep are the net; they already cover source-order, fd>2, heredoc/here-string ordering, stage-0 default, dup/close, and the `line N:` prologue across both modes.

## Risks

- **Large mechanical move.** ~800 lines relocate into `spawn_pipeline`; the risk is a subtle drop or reorder during the move (a pipe not closed, a `parent_held` entry missed, a pgroup step lost). Mitigation: the two pipeline differential audits + `fd_torture` + the sweep are a dense, mode-specific net over exactly this code; `git diff -w` confirms the move is logic-preserving; the implementation is staged (see the plan) so each step is independently green.
- **Heredoc-writer disposition is a correctness-critical `mode` branch.** Getting it wrong reintroduces either a foreground zombie (writer never reaped) or a background hang (writer waited-on inline). The v294 review verified the current split; the merge must preserve it exactly (foreground accumulates + reaps in the wrapper; background drops).
- **Capture-vs-inherit is the other correctness-critical `mode` branch.** Background must keep `ChildFd::Inherit` for the last stage regardless of sink type; letting the foreground capture arms fire in background mode would change `$(a | b &)` (spawn a capture pipe the backgrounded job writes into instead of the terminal). The guard is `mode`, not sink type. The capture-drain lives in the foreground wrapper and consumes the returned `capture_read_fd`/`capture_err_read_fd`; the fg pipeline audit and the command-substitution diff harnesses are the net.
- **Background path remains comparatively under-observed**, but the v294 marker-poll bg audit is now the standing gate and must stay 0 DIVERGE.

## Non-goals

Finishing `RedirectSlot` retirement (#147), the Phase-5 procsub/job-control pgroup alignment (#97/#45), and #144/#145/#78/#79.
