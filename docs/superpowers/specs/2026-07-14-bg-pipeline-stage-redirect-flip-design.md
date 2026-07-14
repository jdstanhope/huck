# Background pipeline-stage redirect flip (P3b background half) — closes #50/#69

**Issues:** [#50](https://github.com/jdstanhope/huck/issues/50) + [#69](https://github.com/jdstanhope/huck/issues/69) — **background half; this iteration CLOSES both.** **Phase:** fd-plumbing remediation **P3b** (background). **Mirrors:** the v293 foreground flip (`docs/superpowers/specs/2026-07-14-pipeline-stage-redirect-flip-design.md`) applied to `run_background_sequence`. **Builds on:** v292's `build_child_redir_plan` + v293's `spawn_external_with_fds(…, plan: Option<ChildRedirPlan>)`.

## Why (one paragraph)

v293 flipped `run_multi_stage` (foreground pipelines) external stages off the lossy last-wins `RedirectSlot` fast-path onto the ordered `build_child_redir_plan` replay, fixing the foreground half of #50 (stage source-order + fd>2 heredoc). `run_background_sequence` (bare trailing-`&` pipelines) is a near-verbatim clone that still carries the same slot path and the same bug. Confirmed vs bash 5.2.21 on a bare background pipeline (`sh -c 'echo O; echo E>&2' 2>&1 >f | cat >o &`): bash writes `O` to the file and `E` to the pipe; huck writes **both to the file** and nothing to the pipe (last-wins slots). v294 applies the identical flip to `run_background_sequence`, closing #50/#69.

## Scope

**In scope (v294):**
- `run_background_sequence` **external** simple-command stages: build a pipe/`stage0_default`-only `ChildStdio` base and replay the stage's full ordered `ChildRedirPlan` in the child — replacing the slot-derived 0/1/2 stdio + dup-targets + `build_child_extra_ops` for that path.
- #69 background half: stamp `current_lineno` before lowering so a background stage redirect-open error carries `line N:`.
- A new differential gate for background pipelines: `tools/bg_pipeline_redirect_audit.sh` (marker-poll, see Testing).

**Explicitly NOT in scope:**
- **InProcess (compound + builtin) background stages** — untouched (their forked subshell body applies redirects via the v292 in-process machinery); the `stdout_dup_target`/`stderr_dup_target` block and the `fork_and_run_in_subshell` call stay.
- **Deleting the now-dead slot machinery** (`slots_for_simple_path`, `RedirectSlot`, `build_child_extra_ops`) — after v294 no pipeline function reads it, but its removal is a separate follow-up (keeps this risky flip's diff tight and reviewable).
- `run_multi_stage` (v293, unchanged); the software-sink layer; job-control/pgroup wiring.
- #144 (builtin-stage stderr routing), #145 (pipeline-abort-on-stage-redirect-failure), #78, #79 — must remain as-is (neither fixed nor regressed).

## Design

### The flip (identical shape to v293, on `run_background_sequence`)

A background pipeline stage's fds are the same two layers as foreground: a structural base (pipe wiring, plus the stage-0 async `/dev/null` default `stage0_default`) carried by `ChildStdio`, and the stage's explicit redirects. Today the explicit redirects go through last-wins slots + `build_child_extra_ops`; the flip routes external stages through the full ordered `ChildRedirPlan` replayed over the base.

Concretely, in `run_background_sequence` (executor.rs ~2845–3600):
1. **Classify first.** Hoist `let stage_is_external = matches!(classify_stage(stage_cmd, shell), StageKind::External(_));` before the `let stdin: ChildFd = …` base construction (~3053).
2. **Guard the three slot-read blocks** (`slot_stdin` ~3053, `explicit_stdout` ~3230, `explicit_stderr` ~3310): change each `if let Command::Simple(SimpleCommand::Exec(exec)) = stage_cmd {` to `… && !stage_is_external {`. External stages then fall to the compound `else` branches, which already produce the pipe-only base AND preserve the stage-0 default (the `else` at ~3196 uses `prev_pipe_read` or `stage0_default.try_clone()`; the stderr `else` yields `ChildFd::Inherit`).
3. **Build `external_plan` before the inter-stage pipe is created.** For external stages, immediately before the "Build stdout fd" section (so a forked heredoc writer cannot inherit the stage's own pipe write end — the v293 deadlock lesson), stamp `if exec.line != 0 { shell.current_lineno = exec.line; }` then `build_child_redir_plan(&exec.redirects, shell, sink, err_sink)`; on `Ok(mut p)` append `p.heredoc_writers` to the loop's `heredoc_writers` accumulator and keep `Some(p)`; on `Err(_)` do the existing `restore_inline_assignments` + `bail_teardown_bg(...)`. `None` for InProcess.
4. **Pass `external_plan` to the spawner.** The `StageKind::External` arm of the spawn `match` (~3557) passes `external_plan` as the final arg (currently `None`, added in v293 T2). The `StageKind::InProcess` arm is unchanged.

Correctness is identical to v293 by construction: `X 2>&1 >f | …` → base pipe-write on fd 1; plan replays `dup2(1,2)` (2→pipe) then `dup2(f,1)` (1→f) ⇒ stderr→pipe, stdout→file. `X >f | …` → the plan's `dup2(f,1)` overrides the inter-stage pipe write end ⇒ downstream reads EOF.

### Applier reuse

No new spawner or lowering code: `build_child_redir_plan`, `redir_plan_to_child`, `replay_redir_ops`, and `spawn_external_with_fds(…, Some(plan))` are exactly as v293 uses them. Only `run_background_sequence`'s base construction + plan-build + spawn arg change.

### The gate — `tools/bg_pipeline_redirect_audit.sh` (marker-poll)

A bare `pipeline &` detaches (the shell exits immediately; the job survives per #128/v289) and writes its result files asynchronously — and a trailing `wait` would route through a different dispatch path (`seq.rest` non-empty), skipping `run_background_sequence`. So the harness runs each case as a bare bg pipeline and OBSERVES via files, not a captured stream:

- Each case: a fragment that runs an **external** stage pipeline (`/bin/sh -c '…'` / `/bin/cat` / `/bin/echo`) writing to one or more result files, ending `… &` (rest empty → `run_background_sequence`). Where the pipeline's stderr matters, the last stage captures it to a file (`… 2>ERRFILE` on the consumer, or the stage redirects to a named file).
- The runner (mirroring `pipeline_redirect_audit.sh`): fresh temp dir per case; run `HUCK -c '<fragment>'` (returns immediately); then **poll** each expected result file until its size is stable across two consecutive reads (~50 ms apart) or a per-case timeout (default 5 s) elapses; a timeout marks the case `TIMEOUT` (a divergence). Compare the full set of result files (contents) bash-vs-huck, normalized for the program-name prefix as in the foreground audit.
- Corpus (each as a bare bg pipeline writing files): ordering `2>&1 >f`, `>f 2>&1`, `>f 2>f2`; fd>2 `3>f`, `4>f`; input `<f`; here-string `<&3 3<<<`; dup/close `3>&1`, `2>&-`; stage-1 vs last-stage redirect.

The gate: **bash == huck on every case** (0 DIVERGE). The bg job's own exit races are absorbed by polling-until-stable, not a fixed sleep.

## Testing

1. **`tools/bg_pipeline_redirect_audit.sh` = 0 DIVERGE** (the fix). On CURRENT code it is RED (shows the background #50 divergences) — build it first, confirm RED, then the flip turns it GREEN.
2. **`tools/pipeline_redirect_audit.sh` (foreground) stays 0 DIVERGE** and **`tools/redirect_audit.sh` (single-command) stays 16 DIVERGE** — the flip must not perturb either.
3. `fd_torture_diff_check.sh` stays green UNCHANGED (do NOT add bg-stage cases: a bare `pipeline &` detaches with no synchronous output, so `fd_torture`'s output-comparison can't test it deterministically, and a `wait` would skip `run_background_sequence`; the bg audit is the correct gate for the bg path). `named_fd_integration` 7/7; engine lib green; full `run_diff_checks.sh` sweep 0-failed on BOTH binaries.
4. **Anti-hang discipline:** every bg case's poll has a per-case timeout; a hang → divergence, never a stall.

## Risks

- The background path is the review's "under-tested" area; the new differential audit is the mitigation.
- Async observation is inherently racier than the synchronous foreground audit — the poll-until-stable (not fixed-sleep) design is the guard; if a case proves flaky, prefer a longer per-case timeout over a fixed sleep.
- The stage-0 `stage0_default` preservation is load-bearing: an external first stage with no explicit stdin must still get `/dev/null` (non-interactive) — the compound `else` branch provides it, but the audit must include a stage-0-no-redirect case to confirm.

## Non-goals

Slot-machinery deletion (follow-up); the Phase-4 `spawn_pipeline` merge; procsub/job-control (Phase 5); #144/#145/#78/#79.
