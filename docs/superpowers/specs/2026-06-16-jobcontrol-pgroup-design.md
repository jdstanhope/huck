# v172: centralize job-control + fix L-51 (non-interactive pipeline process group) — Design

**Status:** approved 2026-06-16
**Iteration:** v172
**Origin:** L-51 — huck `setpgid`s a pipeline into its own process group even when
job control is off (non-interactive), whereas bash keeps the stages in the
shell's process group. Root cause: the job-control predicate is copy-pasted into
4+ fork sites and **drifted** — some include `shell.is_interactive`, the
fork/setpgid ones don't.

## Goal

Centralize the job-control decision into one method (so it can't drift), have the
fork primitives own the `setpgid` policy, and thereby make a non-interactive
shell keep pipeline (and other forked) children in its own process group, matching
bash — without changing interactive job control.

## Problem

Two coupled defects in the fork machinery (`src/executor.rs`):

1. **Drifted predicate.** The job-control test is inlined at several fork sites as
   `matches!(sink, StdoutSink::Terminal) && !in_subshell && !in_completion`
   (run_multi_stage, the `Command::Subshell` arm, run_background_sequence) —
   **omitting `shell.is_interactive`**. The background job-*notice* path instead
   uses `shell.is_interactive && !in_subshell && !in_completion`. So a
   non-interactive shell whose stdout is a terminal sink wrongly takes the
   job-control path.
2. **Unconditional child `setpgid`.** The fork primitives
   (`fork_and_run_in_subshell` child `setpgid(0, pgid_target)` at ~5752; the
   parent race-closes at ~5830 and `spawn_external_with_fds` ~6047) always
   `setpgid`. `pgid_target == 0` means "become a group leader," so even with the
   predicate fixed, a non-interactive pipeline would become *per-stage* groups —
   still not bash (which keeps stages in the shell's group, i.e. NO `setpgid`).

The terminal-handoff and `WUNTRACED`-stopped machinery in run_multi_stage is
already gated on the (drifted) predicate, so once the predicate includes
`is_interactive`, the non-interactive path correctly falls through to plain reap.

## Design (refactor-first; the fix falls out)

### 1. One job-control predicate

```rust
impl Shell {
    /// True when this shell should use job control (own process groups +
    /// terminal handoff) for the commands it forks: an interactive shell that is
    /// not inside a subshell environment or a completion function. The single
    /// source of truth — replaces the drifted inline copies.
    pub fn job_control_active(&self) -> bool {
        self.is_interactive && !self.in_subshell && !self.in_completion
    }
}
```

Foreground fork sites additionally require a terminal sink (a captured `$(a|b)`
must never job-control): they use `shell.job_control_active() && matches!(sink,
StdoutSink::Terminal)`. The background path uses `shell.job_control_active()`
(it has no sink). All the drifted inline predicates are replaced with these.

### 2. The fork primitives own the `setpgid` policy

```rust
/// Sentinel `pgid_target` meaning "do not setpgid — inherit the shell's process
/// group" (job control off). `0` = become a new group leader; `N>0` = join group N.
const NO_PGROUP: i32 = -1;
```

In `fork_and_run_in_subshell` and `spawn_external_with_fds`, gate every
`setpgid(…, pgid_target)` (the child join at ~5752 and the parent race-closes at
~5830/~6047) on `pgid_target >= 0`. With `NO_PGROUP` the child performs no
`setpgid` and therefore inherits the shell's process group (its fork parent).

### 3. Each fork path computes its target uniformly

Replace the per-path `let pgid_target = if interactive { first_pid.unwrap_or(0) }
else { 0 };` with:

```rust
let pgid_target = if interactive { first_pid.unwrap_or(0) } else { NO_PGROUP };
```

where `interactive` is now `shell.job_control_active() && matches!(sink,
Terminal)` (foreground) or `shell.job_control_active()` (background). Applied to:
`run_multi_stage` (the documented L-51 pipeline path), the `Command::Subshell`
arm, the single external/builtin command path (`run_exec_single` →
`spawn_external_with_fds`), and `run_background_sequence`.

### Behavior

- **Job control ON (interactive, terminal sink, not subshell/completion):**
  byte-for-byte unchanged — pipeline gets its own group, terminal handoff,
  `WUNTRACED` stop/`fg` resume all as before.
- **Job control OFF (non-interactive / capture sink / subshell / completion):**
  forked children inherit the shell's process group (no `setpgid`), matching
  bash; the already-gated terminal-handoff/`WUNTRACED` code is skipped (plain
  reap), unchanged.

### Why this is "easier to change in the future"

Job-control *policy* now lives in one method (`job_control_active`); pgroup
*mechanism* lives in the two fork primitives (`pgid_target` ∈ {NO_PGROUP, 0, N}).
A future change to either touches one place and cannot drift across the fork
sites — the exact failure mode that produced L-51.

## Verification

Per-path behavior is verified **against bash** (the pgroup of each fork kind in
interactive vs non-interactive mode); any path where bash diverges from this
model is reconsidered before shipping.

- **New PTY regression test** `tests/jobcontrol_pgroup_pty.rs` (mirroring the
  existing `tests/*_pty.rs` + `expectrl` pattern):
  - *Interactive job control still works:* a pty-backed interactive huck runs a
    pipeline, Ctrl-Z stops it (`jobs` shows Stopped), `fg` resumes it to
    completion.
  - *L-51 fixed (non-interactive):* a non-interactive pipeline's stages share the
    **shell's** process group (assert via `ps -o pgid`), not a sibling group.
- **The existing PTY suite is the deadlock-regression guard** and must stay green:
  `subshell_pipeline_pty`, `subshell_tty_pty`, `subshell_job_notice_pty`,
  `completion_jobcontrol_pty`, `procsub_stop_pty`, `sigint_abort_pty`,
  `pty_interactive` (these encode the v108/v121/v124 fixes).
- The orphan-leak reproduction (a non-interactive pipeline whose stages now stay
  in the shell's group is reachable by a group kill).
- Full unit + integration suite (0 failures), all 93 bash-diff harnesses, clippy
  clean.

## Docs / iteration close-out

Resolves L-51: on merge, **delete** the L-51 entry from
`docs/bash-divergences.md` and decrement the Tier-4 count 40 → 39. Record the
iteration in `project_huck_iterations.md` + `MEMORY.md`.

## Scope boundary

In scope: the `job_control_active` helper, the `NO_PGROUP` sentinel + gated
`setpgid` in the two fork primitives, switching the four fork paths to the
helper, the new PTY test, and the L-51 doc removal. **Not** in scope: a real
`set -m`/`monitor`-driven job-control toggle (huck has the `monitor` shopt
registered but unwired — job control here is keyed on interactivity, as today),
and any change to the terminal-handoff/`WUNTRACED` mechanics themselves (only
their *gating* is corrected, via the centralized predicate). No change to
interactive behavior.
