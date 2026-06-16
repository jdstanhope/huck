# v173: background jobs inherit the shell's process group when job control is off (fix L-53) — Design

**Status:** approved 2026-06-16
**Iteration:** v173
**Origin:** L-53 — v172's follow-on. v172 fixed the FOREGROUND non-interactive
process-group divergence (former L-51); the BACKGROUND paths
(`run_background_subshell` for `( cmd ) &`, `run_background_sequence` for
`a | b &`) still `setpgid` their first stage into a NEW process group
unconditionally, so a non-interactive backgrounded pipeline/subshell lands in a
sibling group instead of inheriting the shell's group like bash.

## Goal

Make non-interactive background jobs (`a | b &`, `( cmd ) &`) keep their stages
in the shell's own process group — matching bash — by reusing v172's
`Shell::job_control_active()` + `NO_PGROUP` machinery, WITHOUT regressing
interactive job control or the non-interactive job-control builtins
(`kill %n`, `wait`, `$!`) that currently rely on the (incorrectly-created) group.

## Verified bash behavior (the reference)

Non-interactive bash keeps background children in the shell's process group:

```
$ bash -c 'cat fifo | wc -c &  sleep .5; ps -eo pid,ppid,pgid,comm | grep -E "cat|wc"'
  <cat>  <shell>  <shell-pgid>  cat
  <wc>   <shell>  <shell-pgid>  wc            # same pgid as the shell
$ bash -c '( exec sleep 5 ) &  sleep .5; ps -o pgid= ...'   # child shares shell pgid
```

And `kill %1` / `wait %1` still work non-interactively and signal ALL stages of
a backgrounded pipeline (bash tracks the job's pids and signals them
individually when the job has no process group of its own — `J_JOBCONTROL`
unset). huck currently passes these only because it (wrongly) gives the job its
own group; once we stop doing that, huck must signal per-pid to keep matching.

huck today (post-v172) diverges: the backgrounded stage gets `pgid == its own
pid` (a sibling group), confirmed by inspection.

## Problem (src/executor.rs + src/jobs.rs + src/builtins.rs + src/shell_state.rs)

Three coupled concerns:

1. **Unconditional background `setpgid`.** `run_background_subshell`
   (`executor.rs` ~1959) calls `fork_and_run_in_subshell(..., pgid_target=0,
   ...)` (child becomes group leader). `run_background_sequence` (~1986) sets
   each stage's `pgid_target = first_pid.unwrap_or(0)` (~2043 assign-stage,
   ~2365 general-stage) AND the parent race-closes with a hardcoded
   `setpgid(pid, pid)` at two sites (~2078 assign-stage, ~2469 general-stage),
   gated only on `first_pid.is_none()`. All of this fires even when job control
   is off.
2. **`kill %n` assumes a group.** `builtin_kill`'s job-spec arm (`builtins.rs`
   ~4574) does `killpg(job.pgid, sig)`. With no group, `job.pgid` (= first pid)
   is not a valid process-group id → `killpg` returns ESRCH and the signal
   reaches nothing. bash signals the job's pids individually in this case.
3. **Partial-cleanup hang.** `cleanup_partial_pipeline_raw` (~2521) does
   `killpg(first_pid, SIGKILL)` then a BLOCKING `waitpid(pid, …, 0)` per pid.
   With no group, the `killpg` is a no-op (ESRCH), so the already-spawned
   stages are never killed and the blocking `waitpid` can deadlock the shell on
   the spawn-failure error path.

`wait` / `wait %n` are NOT affected: `builtin_wait` / `wait_for_job` reap via
`waitpid(-1, WNOHANG)` and match against `job.pids` (per-pid), never
`waitpid(-pgid)`. `fg` / `bg` (`killpg`, `tcsetpgrp(-pgid)`,
`waitpid(-pgid)`) operate only on STOPPED jobs, which arise solely from
interactive Ctrl-Z — those jobs always own their group, so those paths are
unaffected.

## Design

### 1. Gate the background `setpgid` on job control (reuse v172)

In both background functions compute, before spawning:

```rust
let job_control = shell.job_control_active();
```

(Background has no `StdoutSink` foreground/terminal distinction — the helper
alone is the predicate, as the v172 spec specified for the background path.)

- `run_background_subshell`: pass `if job_control { 0 } else { NO_PGROUP }` as
  the `fork_and_run_in_subshell` `pgid_target` argument.
- `run_background_sequence`: at both per-stage `pgid_target` computations use
  `if job_control { first_pid.unwrap_or(0) } else { NO_PGROUP }`; and gate each
  parent race-close `setpgid(pid, pid)` (the `if first_pid.is_none()` blocks at
  ~2078 and ~2469) additionally on `job_control` — when off, skip the parent
  `setpgid` entirely (the child also skips it, since the fork primitives are
  already `pgid_target >= 0`-gated from v172).

Net: job control ON (interactive) → byte-for-byte unchanged (first stage is the
group leader, later stages join it, `job.pgid` = leader pid). Job control OFF →
no `setpgid` anywhere; all stages inherit the shell's process group, matching
bash.

### 2. Track whether a job owns its process group

Add a field to `Job` (`src/jobs.rs`):

```rust
pub own_pgroup: bool,   // true: job has its own process group (killpg-able);
                        // false: job shares the shell's group (signal per-pid)
```

Keep `JobTable::add(pgid, pids, command)`'s signature (it defaults
`own_pgroup = true`), so the 30+ existing call sites — all interactive /
own-group paths or unit tests — are untouched. Add a sibling:

```rust
pub fn add_with_pgroup(&mut self, pgid: i32, pids: Vec<i32>, command: String,
                       own_pgroup: bool) -> u32
```

(`add` delegates to it with `true`.) The two background functions call
`add_with_pgroup(..., job_control)`.

`builtin_kill`'s job-spec arm branches on the resolved job's `own_pgroup`:

```rust
if job.own_pgroup {
    libc::killpg(job.pgid, sig)          // existing behavior
} else {
    for &pid in &job.pids { libc::kill(pid, sig); }   // signal each stage
}
```

(Match bash's rc/diagnostics: success when every per-pid `kill` succeeds.)

### 3. Fix the partial-cleanup hang

`cleanup_partial_pipeline_raw` additionally `kill(pid, SIGKILL)`s each
individual pid before the blocking reap (the pids are the shell's direct
children, so this is always valid). The existing `killpg(pg, SIGKILL)` is kept
(harmless ESRCH when no group; still catches grandchildren when a group
exists). No signature change — works for both job-control states.

### 4. `hangup_jobs` (SIGHUP on exit)

`Shell::hangup_jobs` branches on `own_pgroup`: `killpg(pgid, …)` when the job
owns its group, else `kill(pid, …)` per pid. Best-effort (errors still
ignored). Keeps SIGHUP-on-exit reaching non-grouped background jobs.

### Behavior summary

- **Interactive (job control on):** every path unchanged — own-group jobs,
  `killpg`/`tcsetpgrp` job control, `fg`/`bg`/`kill %n` all as before.
- **Non-interactive (job control off):** `a|b &` and `(cmd) &` stages inherit
  the shell's process group (bash-match); `kill %n` signals the job's pids
  individually; `wait`/`$!` unchanged; the partial-spawn error path kills its
  children directly instead of hanging.

## Verification

Per-path behavior verified against bash (the pgroup of each background kind in
interactive vs non-interactive mode):

- **PTY/process tests** (extend `tests/jobcontrol_pgroup_pty.rs`): non-interactive
  `a | b &` stages and `( cmd ) &` child share the shell's pgid (assert via
  `ps -o pgid`); the existing interactive Ctrl-Z/`fg` pipeline test stays green.
- **bash-diff harness** `tests/scripts/bg_jobcontrol_diff_check.sh` for the
  OBSERVABLE behavior (pgids aren't byte-stable, so assert outputs/exit codes):
  non-interactive `sleep & echo $!; kill %1; wait` and a backgrounded pipeline
  `kill %1` produce bash-identical stdout + exit status.
- **Existing PTY suite is the deadlock guard** and must stay green:
  `subshell_pipeline_pty`, `subshell_tty_pty`, `subshell_job_notice_pty`,
  `completion_jobcontrol_pty`, `procsub_stop_pty`, `sigint_abort_pty`,
  `pty_interactive`, `jobcontrol_pgroup_pty`.
- Full unit + integration suite (0 failures), all bash-diff harnesses, clippy
  clean.

## Docs / iteration close-out

Resolves L-53: on merge, **delete** the L-53 entry from
`docs/bash-divergences.md` (Tier-4 count 40 → 39). Record the iteration in
`project_huck_iterations.md` + `MEMORY.md`.

## Scope boundary

In scope: gating the background `setpgid` on `job_control_active()` + `NO_PGROUP`
(both background functions), the `Job.own_pgroup` flag + `add_with_pgroup`, the
`kill %n` per-pid fallback, the `cleanup_partial_pipeline_raw` per-pid kill, the
`hangup_jobs` per-pid branch, and the tests + L-53 doc removal. **Not** in
scope: any change to interactive job control, `fg`/`bg`, the foreground paths
(v172), or a real `set -m` toggle (job control remains keyed on interactivity).
No new job-control features — only making the non-interactive background pgroup
and its dependent signaling match bash.
