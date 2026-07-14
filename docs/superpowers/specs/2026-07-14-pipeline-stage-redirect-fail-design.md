# A pipeline stage's redirect failure fails only that stage (#145)

**Issue:** [#145](https://github.com/jdstanhope/huck/issues/145) — a pipeline STAGE's redirect-setup failure currently aborts the whole pipeline (rc 1); bash reports the error, fails just that stage, and runs the remaining stages (pipeline rc = last stage). **Enabled by:** v295 (P4) — pipeline execution is now the single `spawn_pipeline`, so this is a one-site fix. **Not a numbered fd-phase**; a pipeline-semantics correctness fix surfaced by the v293 P3b scoping.

## Why (one paragraph)

In `spawn_pipeline`'s stage loop, every stage-setup error branch does `return Err(bail_teardown_pipeline(...))`, tearing down the entire pipeline. bash treats a per-stage redirect failure as a failure of *that stage only*: the failed stage is a process that prints the error and immediately exits 1, wired into the pipe topology, so downstream stages read EOF, upstream stages that fed the failed reader die with SIGPIPE (141), the failed stage's `$PIPESTATUS` entry is 1, and the pipeline's exit status is the last stage's. Confirmed vs bash 5.2.21: `echo A | cat </no/such/file | cat` → bash rc 0, `PIPESTATUS=(0 1 0)`; huck rc 1, `PIPESTATUS=()`. The bug affects **external** stages (parent-side `build_child_redir_plan`) and **builtin** stages (parent-side slot opens); **compound** stages already behave correctly because they apply their redirects inside the forked subshell.

## Bash semantics (the target behavior)

| case | bash rc | bash `$PIPESTATUS` |
|---|---|---|
| `echo A \| cat </no \| cat` (middle fails) | 0 | `(0 1 0)` |
| `cat </no \| wc -c` (first fails) | 0 | `(1 0)` |
| `echo A \| cat </no` (last fails) | 1 | `(0 1)` |
| `cat </no/a \| cat </no/b \| wc -c` (two fail) | 0 | `(1 1 0)` |
| `yes \| head -1000 \| cat </no \| wc -l` (upstream floods a dead reader) | 0 | `(141 141 1 0)` |
| `yes \| read x </no \| cat` (failed stage redirects stdin AWAY from the pipe) | — | `(141 1 0)` |

The failed stage's exit code is **1** for every redirect failure (missing file, bad-fd `<&7`, permission). Upstream SIGPIPEs (141) even when the failed stage redirected its stdin away from the inter-stage pipe (the pipe read end still closes because the stage never reads it). The redirect-error message is printed **once**, by the existing setup code, before the failure is handled.

## Scope

**In scope (v296):**
- The four stage-*own-redirect*-setup failure sites in `spawn_pipeline` become "fail this stage, continue the pipeline": the builtin stdin-slot open, the `explicit_stdout` open, the `explicit_stderr` open, and the external `build_child_redir_plan`.
- A new `spawn_failed_stage` helper that forks a child wired to the stage's inter-stage pipe ends and immediately `_exit(1)`.
- Covers external + builtin stages, foreground + background (all one `spawn_pipeline`).

**Explicitly NOT in scope:**
- **Infrastructure failures** — `make_pipe` / `make_orphan_pipe_for_eof_reader` (cannot create an inter-stage pipe) keep aborting the whole pipeline (bash does too — a resource failure is fatal). Only a stage's *own redirect* failure continues.
- **Spawn/exec failures** (`fork()` returns -1; an external's `exec` fails) — the domain of #78; different code path, separate message/leak concerns. `spawn_failed_stage` is exactly the primitive #78 will later reuse, but v296 does not touch the spawn path.
- **Compound stages** — already correct (redirects applied in the forked subshell); untouched.
- The `RedirectScope` in-process apply path, redirect lowering, the sink layer (#137/#140/#141/#142/#144 untouched).

## Design

### The restructure: don't early-return on a stage's own redirect failure

The stage loop today runs, per stage: build stdin (phase 1) → determine `explicit_stdout` (phase 2) → determine `explicit_stderr` (phase 3) → build external `ChildRedirPlan` (phase 4, external only) → build stdout / inter-stage pipe (phase 5) → build stderr (phase 6) → classify + spawn (phase 7) → post-spawn pid tracking. Phases 1–4 are where a stage's own redirect can fail; each currently `return`s a bail.

Introduce a per-iteration `let mut redirect_failed = false;`. At each of the four redirect-setup failure sites, instead of `return Err(bail_teardown_pipeline(...))`: the error is already printed, so drop any partially-opened fd, set `redirect_failed = true`, and fall through. Guard the subsequent redirect-*opening* phases so a stage does not keep opening redirects after the first failure (matching bash's first-failure-wins, and avoiding a second error message):
- Phase 2/3 (`explicit_stdout`/`explicit_stderr`) and phase 4 (external plan) are guarded `if !redirect_failed { … }`. In practice a builtin stage can only fail in phases 1–3 (its slot opens) and an external stage only in phase 4 (its plan) — the two kinds never both fail — but the guards make the fall-through uniform and future-proof.
- Phase 5 (build stdout / inter-stage pipe) runs **regardless** of `redirect_failed`: the failed stage still needs its outgoing inter-stage pipe so downstream reads EOF. When `redirect_failed`, `explicit_stdout` is `None` (its open was skipped or dropped), so phase 5 takes its normal `else if !is_last { make_pipe }` path and builds the pipe. A `make_pipe` failure here still bails (infrastructure).

At phase 7, branch the spawn:
```rust
let pid = if redirect_failed {
    // The stage's own redirect setup failed; the error is already printed.
    // Fork a child wired to this stage's inter-stage pipe ends that exits 1,
    // so downstream reads EOF, upstream (if it fed this stage) gets SIGPIPE,
    // and $PIPESTATUS records 1 — matching bash's per-stage failure.
    match spawn_failed_stage(shell, child_stdio, pgid_target, &fds_to_close_in_child) {
        Ok(pid) => pid,
        Err(_) => { /* fork failed => genuine infrastructure failure */
            restore_inline_assignments(snap, shell);
            return Err(bail_teardown_pipeline(mode, shell, procsub_base, first_pid, &stages, &mut parent_held));
        }
    }
} else {
    match spawn_result { /* existing External / InProcess spawn handling */ }
};
```
Then the **existing** post-spawn tracking runs unchanged for `pid` (first_pid / live-pid registry / `stages.push(PipelineStage::Forked(pid))` / the parent's pipe-end close bookkeeping / `prev_pipe_read` update). The failed stage is an ordinary rc-1 child in every downstream mechanism — no new `PipelineStage` variant, no `wait_pipeline_raw` / `$PIPESTATUS` special-casing.

`child_stdio` for the failed stage is `ChildStdio::new(stdin, stdout, stderr)` built from the same phase-1/5/6 locals as a normal stage — which, because the failed redirect was skipped, are exactly the pipe-based base (stdin = `prev_pipe_read` clone or `Inherit`; stdout = the inter-stage pipe or `Inherit` for the last stage; stderr = `Inherit`). No special base construction.

**Phase-1 stdin-failure fallback.** The `slot_stdin()` `Read` path first `take()`s and **closes** `prev_pipe_read` (discarding the upstream pipe, as it does today) and then opens the file. If that open fails, `prev_pipe_read` is already closed — so the upstream stage's write end has no reader and SIGPIPEs on its own, without the dummy needing a stdin pipe. In that branch, set the `stdin` local to `ChildFd::Inherit` (a valid value so the `let stdin` binding is initialized), set `redirect_failed = true`, and fall through; the dummy's stdout inter-stage pipe (phase 5) still gives downstream EOF. For a phase-2/3/4 failure, `prev_pipe_read` was not consumed by the failing redirect, so `stdin` is the normal pipe read end and the dummy's exit closes it → upstream SIGPIPE. Either way the upstream-SIGPIPE and downstream-EOF semantics hold.

### `spawn_failed_stage`

```rust
/// Fork a "failed pipeline stage": a child that inherits this stage's stdio
/// (its inter-stage pipe ends), joins the job's process group, closes every
/// other parent-held pipe fd, and immediately _exit(1). It runs no command and
/// prints nothing (the redirect error was already reported by the parent). Its
/// exit closes the pipe ends it holds, giving downstream EOF and upstream
/// SIGPIPE — reproducing bash's per-stage redirect failure. Returns the child pid.
fn spawn_failed_stage(
    shell: &mut Shell,
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
) -> Result<i32, io::Error>
```

It mirrors `fork_and_run_in_subshell`'s child prologue but does **not** need the dup2-to-0/1/2 install: the child runs no command, so it only has to *hold* its stdio pipe ends open until `_exit` (which closes them) and close every unrelated parent pipe fd (so it doesn't wedge other stages' EOF). Concretely, in the child: `flush_stdout()` (parent buffer already flushed by the caller if needed), `setpgid(0, pgid_target)` when `pgid_target >= 0`, take the three `ChildStdio` raws (keep them open — do not close), close each fd in `parent_fds_to_close` that is not one of those three raws, then `libc::_exit(1)`. Parent: the defensive `setpgid(pid, pgid_target)` race-close (as the other spawners do), then `Ok(pid)`. Flush the parent's buffered stdout before the fork (as `fork_and_run_in_subshell` does) so no pending bytes are duplicated into the child.

The child-side "convert `ChildStdio` to raws + close the other parent fds" logic is small; if it can share a helper with `fork_and_run_in_subshell` cleanly, do so, but a focused duplication (≈10 lines, no dup2 passes) is acceptable — the two children differ (one installs stdio and runs a body, the other only holds fds and exits).

### Which sites change (exact)

Continue (set `redirect_failed`, fall through):
1. Builtin stdin-slot open failure — the `slot_stdin()` `Read`/`Heredoc`/`HereString` open in phase 1 (`~5988`).
2. `explicit_stdout` open failure — phase 2 (`~6196`).
3. `explicit_stderr` open failure — phase 3 (`~6282`).
4. External `build_child_redir_plan` `Err` — phase 4 (`~6386`).

Still bail (infrastructure — unchanged):
- `make_pipe` for an inter-stage pipe (phase 5, `~6452`) and the capture-sink pipe (`~6489`).
- `make_orphan_pipe_for_eof_reader` (phase 5, `~6420`).
- `fork()` failure inside `spawn_failed_stage` (a genuine resource failure).

## Testing

1. **New `tests/scripts/pipeline_stage_redirect_fail_diff_check.sh`** — a bash-vs-huck byte-identical harness (stdout + stderr normalized for the shell-name prefix, plus `echo "rc=$? PIPESTATUS=(${PIPESTATUS[@]})"`) over the matrix:
   - external stage fails at first / middle / last position;
   - builtin stage fails (`read x </no`) at first / middle / last;
   - compound stage fails (`{ cat; } </no` — regression guard that it still works);
   - two stages fail (`cat </no/a | cat </no/b | wc -c`);
   - upstream-floods-dead-reader SIGPIPE (`yes | head -1000 | cat </no | wc -l` → `(141 141 1 0)`);
   - failed stage redirects stdin away from the pipe (`yes | read x </no | cat` → `(141 1 0)`);
   - bad-fd source-order (`cat <&7 | cat`, and the issue's `/bin/cat <&3 3<<<'HS' | cat`);
   - **background** forms observed via files (a bare `… &` writing result files, mirroring `bg_pipeline_redirect_audit.sh`), confirming a bg pipeline also continues.
   Gate: byte-identical to bash on every case.
2. **`tools/pipeline_redirect_audit.sh` stays 0 DIVERGE** and **`tools/bg_pipeline_redirect_audit.sh` stays 0 DIVERGE** — the fix must not perturb the passing cases. NOTE: the bg audit's `last redir` / `ord 2>&1 >f` cases do not fail redirects, so they stay green; if any audit case's rc/PIPESTATUS was masked by the old abort, it should now MATCH bash (an improvement) — reconcile, don't force.
3. **`tools/redirect_audit.sh` stays 16 DIVERGE** (single-command; #145 is pipeline-only, untouched).
4. `fd_torture_diff_check.sh` (44), engine lib green, `named_fd_integration` 7/7, `bg_pipeline_line_number_integration` 1/1; full `run_diff_checks.sh` sweep 0-failed on BOTH binaries.
5. **Anti-hang discipline:** every harness case runs under `timeout`; a hang (e.g. a mis-wired pipe end that never EOFs) is a divergence, never a stall.

## Risks

- **Pipe-end wiring at the failure point is the crux.** The failed stage must hold exactly its own two inter-stage pipe ends (stdin-from-prev, stdout-to-next) and close all others, or downstream won't EOF / upstream won't SIGPIPE / an unrelated stage wedges. The new diff-check's SIGPIPE (141) and two-fail cases are the specific net for this; a mis-wire shows as a hang (caught by `timeout`) or a wrong PIPESTATUS.
- **Ordering constraint preserved:** the external `build_child_redir_plan` is still built *before* the inter-stage pipe (the v293 heredoc-writer-inherits-the-pipe deadlock). On its failure, no heredoc writer is wired into a live pipe (the plan failed); phase 5 then builds the pipe for the dummy. (A heredoc writer partially forked before a later failing redirect is the pre-existing #142, out of scope.)
- **The restructure touches the ~1150-line stage loop** (guards on phases 2–4, the phase-7 branch). It is localized and behavior-preserving for the success path; the pipeline audits + `fd_torture` + the sweep are the regression net, and `git diff -w` should show the success-path loop body unchanged.

## Non-goals

Spawn/exec-failure continuation (#78), the in-process/lowering/sink divergences (#137/#140/#141/#142/#144), and any change to compound-stage or single-command redirect behavior.
