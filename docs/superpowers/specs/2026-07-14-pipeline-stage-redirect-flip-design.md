# Pipeline-stage redirect flip — retire the `RedirectSlot` fast-path for foreground external stages (Approach A)

**Issue:** [#50](https://github.com/jdstanhope/huck/issues/50) (redirect source-order not preserved on pipeline stages) — foreground half. Also addresses [#69](https://github.com/jdstanhope/huck/issues/69) (foreground half). **Phase:** fd-plumbing remediation **P3b** (review → `docs/superpowers/reviews/2026-07-13-engine-fd-plumbing-review.md`; P3a shipped as v292). **Builds on:** the v292 child-lowering machinery (`build_child_redir_plan` / `redir_plan_to_child` / `ChildRedirPlan` / `replay_redir_ops`).

## Why (one paragraph)

`run_multi_stage` wires each stage's fds through the **lossy `RedirectSlot` fast-path**: `slots_for_simple_path` collapses the stage's redirects into last-wins 0/1/2 slots, and everything the slots drop (fd>2, `<&`/`>&` dups, `N>&-`, `<>`, and — critically — **fd>2 heredocs**) is bolted on afterward via `build_child_extra_ops`. Because the slots are last-wins and the extra ops run *after* the slot stdio, source order is not preserved and fd>2 heredocs are silently dropped. Confirmed vs bash 5.2.21 on external stages: `sh -c 'echo OUT; echo ERR>&2' 2>&1 >f | cat` sends **both** streams to the file in huck (bash: ERR→pipe, OUT→file); and a dropped fd>2 heredoc can **hang** (`/bin/cat <&3 3<<EOF | cat` hung indefinitely). This is exactly the residual **#50**. v292 already built the correct ordered child-lowering (`build_child_redir_plan`) used by single external commands; P3b routes foreground **external pipeline stages** through it too.

## Scope

**In scope (v293):**
- `run_multi_stage` (foreground pipelines) **external** simple-command stages: lower the stage's *full ordered* `Redirection` list via `build_child_redir_plan` and replay it in the child over the structural pipe/capture base — replacing the slot-derived 0/1/2 stdio + dup-target pre-resolution + `build_child_extra_ops` for that path.
- **#69 foreground half:** thread `current_lineno` into the stage redirect-open error path so a stage `>badpath` carries `line N:` (the single lowering site is the natural place).

**Explicitly NOT in scope (do NOT change):**
- **InProcess (compound) pipeline stages** — already correct: the forked subshell body re-applies its redirects through the v292 in-process ordered machinery (verified: `{ …; } 2>&1 >f | cat` and `{ cat <&3; } 3<<EOF | cat` both match bash). The stage-stdio base construction for InProcess stages is unchanged, and `fork_and_run_in_subshell` keeps its `stdout_dup_target`/`stderr_dup_target` params. **InProcess *builtin* stages** (a `Command::Simple(Exec)` that classifies as a builtin) have a *separate*, pre-existing divergence — the builtin's software-sink stderr is not routed to the stage's redirect target (`printf '%d' abc 2>&1 >f | cat` sends the error to the terminal, not the pipe). That is the H7 software-sink layer, **not** the fd slot/plan path this flip changes; filed as [#144](https://github.com/jdstanhope/huck/issues/144) and out of scope here. The flip must neither fix nor regress it.
- **`run_background_sequence`** (background bare-`&` pipelines) — the near-verbatim slot clone. Flipped in **v294**; the shared slot machinery (`slots_for_simple_path`, `RedirectSlot`, `build_child_extra_ops`) therefore **stays** this iteration.
- **#78** (stage spawn-failure leak + `command not found` message/routing) — a separable message/routing concern; its own iteration.
- The software-sink layer (`StdoutSink`/`StderrSink`, `Merged`/`Capture` routing) and job-control/pgroup wiring.

**Close status:** #50 and #69 describe *all* pipeline stages (foreground + background). v293 fixes the **foreground** half; the PR **references** them (does not `Closes`). **v294** flips `run_background_sequence` and closes both.

## Design

### The composition (Approach A)

A pipeline stage's fds are two independent layers:
1. **Structural base** (from the pipeline shape, not the AST): stdin ← previous stage's pipe read end; stdout → next stage's pipe write end, or the capture/orphan pipe, or the terminal; stderr → `Merged`/`Capture` sink dup or the terminal. This is carried by `ChildStdio` and is **unchanged**.
2. **The stage's explicit redirects** (`exec.redirects`, the ordered `Redirection` list): applied left-to-right *on top of* the base, exactly as bash applies redirections after pipe setup.

Today layer 2 is split across last-wins slots + `build_child_extra_ops`. Approach A replaces that split, **for external stages only**, with a single ordered `ChildRedirPlan` from `build_child_redir_plan(&exec.redirects, …)` — the identical lowering a single external command uses — replayed by `replay_redir_ops` in a `pre_exec` after the base `ChildStdio` is installed.

Correctness follows by construction (child fd table, in order):
- `cmd 2>&1 >f | c` → base: pipe-write on 1. plan `[Dup 2←1, Owned 1←f]` → `dup2(1,2)` (2=pipe), `dup2(f,1)` (1=f). ⇒ **stdout→f, stderr→pipe** (the #50 fix).
- `cmd >f 2>&1 | c` → plan `[Owned 1←f, Dup 2←1]` → 1=f, 2=f. ⇒ both→f, pipe EOF (bash).
- `cmd 3<<EOF … | c` → plan installs the heredoc read end on fd 3 (no longer dropped; no hang).
- `cmd <f | c` → base: pipe-read on 0. plan `[Owned 0←f]` → 0=f; the incoming pipe has no reader ⇒ the previous stage SIGPIPEs (bash).

### Changes to `run_multi_stage`

**Classify first.** `classify_stage` currently runs at spawn time, *after* the stdio base is built. Because a `Command::Simple(Exec)` stage can be **either** External (real program) **or** InProcess (a builtin), the base-construction branch cannot key off Simple-vs-Compound — it must key off the **classification**. Hoist `classify_stage(stage_cmd, shell)` to before the stdio-base construction, and branch the base on External vs InProcess. InProcess stages (compound AND builtin) keep the **existing** slot-base construction untouched (their bodies apply redirects; #144 is a separate sink issue). Only External stages get the new pipe-only base + plan.

**Stage-stdio construction (the `slot_stdin`/`slot_stdout`/`slot_stderr` blocks):** For an **external** stage, build the base from the **pipe/capture wiring only** — do NOT read `slot_stdin/stdout/stderr` to open files, spawn heredoc writers, or resolve dups:
- stdin base = `prev_pipe_read` (or `Inherit`); NOT a slot-opened file/heredoc.
- stdout base = next-stage pipe write / capture / orphan pipe / terminal; NOT a slot-opened file.
- stderr base = `Merged`/`Capture` dup or terminal; NOT a slot-opened file/dup.

The prior "discard `prev_pipe_read` when stdin is slot-overridden" special-case is no longer needed: when the plan installs a file on fd 0, the incoming pipe read end (base) is consumed by the child's stdio install and then overridden, and the parent closes its copy after spawn as usual — the previous stage sees no reader and SIGPIPEs, matching bash. (Verify this equivalence in testing.)

InProcess (compound) stages keep the existing base construction and the `stdout_dup_target`/`stderr_dup_target` resolution unchanged.

### Changes to `spawn_external_with_fds`

Replace the slot-derived redirect handling with the full plan:
- **Remove**: the `slot_stdout()`/`slot_stderr()` `Dup` pre-resolution (`stdout_dup_target`/`stderr_dup_target`) and its dedicated dup `pre_exec`; the `build_child_extra_ops` call and its `extra_ops`/`extra_held`/`extra_targets` bookkeeping.
- **Add**: accept the stage's `ChildRedirPlan` (built by the caller via `build_child_redir_plan(&exec.redirects, shell, sink, err_sink)`), install the base `ChildStdio`, then `pre_exec(replay_redir_ops(&plan.ops))`. `plan.held` keeps parent-opened files/heredoc read ends alive (FD_CLOEXEC) until after spawn; `plan.heredoc_writers` are returned to the caller to reap after the stage finishes (same as single-external via `run_subprocess`).
- **`fds_to_close_in_child` / `extra_targets`**: the child must not close a fd a plan op installs. Derive the plan's target set from `plan.ops` (`Dup{target}`/`Close{target}`) and exclude those from the close list, exactly as `extra_targets` did — just sourced from the full plan now.

The plan is built in the **parent** (file opens / heredoc spawns are not async-signal-safe) and replayed in the child's `pre_exec` — the established v292 pattern.

### #69 — `line N:` on stage redirect-open errors

`build_child_redir_plan`'s file-open failures route through `redir_open_error`, which emits `error_prefix(None)` — but that only includes `line N:` when `shell.current_lineno > 0`. `run_multi_stage` must ensure `current_lineno` is set to the pipeline's line before lowering a stage's redirects (thread it the way single-command execution does). Foreground stages then carry `line N:`; the background half remains for v294.

### What is deleted / retained

- **Deleted** (for the external-stage path): the external-stage `slot_stdin/stdout/stderr` open blocks in `run_multi_stage`, the external-stage dup-target pre-resolution, and the `build_child_extra_ops` call in `spawn_external_with_fds`.
- **Retained** (still used by `run_background_sequence` and InProcess stages, until v294): `slots_for_simple_path`, `RedirectSlot`, `build_child_extra_ops`, and the InProcess dup-target params on `fork_and_run_in_subshell`.

## Testing

**Acceptance gate — a NEW pipeline-stage differential audit** (`tools/pipeline_redirect_audit.sh`, mirroring v292's `redirect_audit.sh`): run pipeline-stage redirect constructs through bash 5.2.21 and huck, comparing combined stdout+stderr, exit code, and any target-file contents. Corpus (external stage as `sh -c '…'` / `/bin/cat …` so a real fork is exercised), each in a `… | cat` pipeline:
- ordering: `2>&1 >f`, `>f 2>&1`, `2>&1`, `>f`, `1>&2`, `>f 2>f2`;
- dup/close/move: `3>&1`, `2>&-`, `<f`, `<>f`;
- fd>2 + heredoc: `3<<EOF`, `3<<<w`, `4>f`, `cat <&3 3<<EOF` (the former hang);
- multi-stage: redirect on stage 1 / stage 2 / last stage; capture context `x=$(… 2>&1 >f | cat)`.
The audit must show **bash == huck** on every case (no divergence). Also run the existing `redirect_audit.sh` (single-command) and confirm it is **unchanged at 16** (this iteration must not perturb the single-command path).

**Regression nets:** the existing pipeline diff-tests (`tests/scripts/*_diff_check.sh`) and `fd_torture_diff_check.sh` stay green on both binaries via `run_diff_checks.sh`; `named_fd_integration` 7/7; engine lib green. Add a few `fd_torture` cases pinning the external-stage ordering + the fd>2-heredoc-no-hang fix.

**Anti-hang discipline:** every audit/harness case that could block a reader (`cat`, `<&3`) runs under a `timeout`; a hang is a failure, not a stall.

## Risks

- **Highest-risk phase in the remediation** (per the review) — pipeline redirects are heavily diff-tested, which is the safety net, now reinforced by the new differential audit.
- The external/InProcess base-construction split must stay clean: only the external branch loses its slot opens. A stage misclassified, or an InProcess stage accidentally routed through the plan, would double-apply or drop redirects — the audit's multi-stage + capture cases guard this.
- The pipe-override-by-explicit-stdin equivalence (dropping the `prev_pipe_read.take()` special-case) is a behavioral change in mechanism; the audit's `<f | cat` case and a "previous stage SIGPIPEs" case must confirm bash parity.

## Non-goals

`run_background_sequence` flip + closing #50/#69 (v294); #78; the Phase-4 `spawn_pipeline` merge; procsub/job-control (Phase 5).
