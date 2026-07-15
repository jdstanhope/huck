# v299 — job-control Stopped/Running state reflection (#158) — Design

**Issue:** [#158](https://github.com/jdstanhope/huck/issues/158) — `kill -STOP` / `kill -s CONT`
don't update a job's Running/Stopped state in `jobs`.

**Scope: (a) only** — the core state correctness. Deferred to their own follow-ups:
(b) bash's async job-status *notifications* in non-interactive `set -m` mode, and (c) the
related job-spec gaps (bare `%job` as a resume command, `%?substr` matching).

## Problem

`kill -STOP %1` then `jobs` shows `Running` in huck; bash shows `Stopped`. Repro:

```
huck -c 'set -m; sleep 5 & kill -STOP %1; sleep 1; jobs; kill -9 %1'
#   huck: [1]+ Running   sleep &     (argless render is the separate #80)
#   bash: [1]+  Stopped  sleep 5
```

`kill -s CONT %1` likewise never flips a job back to Running.

## Root cause (verified)

huck already has the full state model — `JobState::{Running, Stopped(i32), Done(i32)}`
(`jobs.rs:13`), `Stopped` rendering (`jobs.rs:343`), `stopped_id()`, and the `bg`/`fg`/`jobs -s/-r`
filters that read it. `reap_completed` (`jobs.rs:298`) already reaps with `WNOHANG | WUNTRACED`
and `reap()` (`jobs.rs:126`) already transitions a job to `Stopped` on `WIFSTOPPED`. The gap is
purely **where the reap runs**:

- The only callers of `reap_completed`/`reap_and_notify` are the interactive REPL's pre-prompt hook
  (`huck-cli/src/repl.rs:186`) and the `wait` builtin (`builtins.rs:4425`).
- In **non-interactive `-c`/script mode** (how the bash test-suite runs `jobs.tests`) nothing
  drains the `WUNTRACED` stop report, and `builtin_jobs` reads `shell.jobs.iter()` **without
  reaping first** (`builtins.rs:4242`). So the stop event is never observed → state stays `Running`.
- Separately, `reap_completed` does not pass `WCONTINUED`, and `reap()` has no `WIFCONTINUED` arm,
  so a `kill -s CONT` is never observed either.

## Design

### Approach A — reap-at-entry in the job-control reader builtins (chosen)

Each job-state reader observes pending STOP/CONT before it reads/acts on state. Surgical: no change
to Done-job cleanup timing, `$!`, `wait`, or PIPESTATUS. (Rejected alternative B — an engine-level
per-statement reap in the non-interactive loop — is more bash-faithful but has a broad blast radius
on Done-job cleanup/`$!`/`wait` timing, unjustified for scope (a).)

### Section 1 — State transitions (`crates/huck-engine/src/jobs.rs`)

1. **`reap_completed` flags:** change the `waitpid(-1, …, WNOHANG | WUNTRACED)` at `jobs.rs:304` to
   `WNOHANG | WUNTRACED | WCONTINUED`, so a continued child is reported.
2. **`reap()` continued arm:** add a `libc::WIFCONTINUED(raw_status)` branch (mirroring the existing
   `WIFSTOPPED` branch at `jobs.rs:129-142`): if the matched job is currently `Stopped`, set it to
   `JobState::Running` and `notified = false`; if already `Running`, no-op (idempotent). Place the
   `WIFCONTINUED` check alongside `WIFSTOPPED`, before the normal-exit reaping logic, and `return`
   after handling (a continued report is not a terminal reap — do not mark `job.reaped[idx]`).

Everything else in `jobs.rs` (Stopped rendering, `stopped_id`, `has_active`, filters) already
handles the states and needs no change.

### Section 2 — Reap wiring (`crates/huck-engine/src/builtins.rs`)

Call `crate::jobs::reap_completed(shell)` at the **entry** of the three job-state readers before
they read state:

- `builtin_jobs` (`builtins.rs:4242`) — so `jobs`, `jobs -s`, `jobs -r`, `jobs -l`, `jobs -p` show
  current state.
- `builtin_bg` — so `bg` finds a newly-stopped job (`stopped_id`) to resume.
- `builtin_fg` — so `fg` acts on current state.

`reap_completed` is non-blocking (`WNOHANG`) and idempotent; calling it at reader entry is cheap and
side-effect-free beyond updating `job.state`. The interactive REPL's pre-prompt reap and the `wait`
builtin's reap are unchanged (the new entry reaps are harmless there — the flag is already cleared /
nothing pending). This is the entire functional fix; all downstream code already reads `job.state`.

### Error handling

No new error paths. `reap_completed` swallows `ECHILD`/no-change as before. State transitions are
idempotent (a duplicate stop/continue report does not re-fire or corrupt state). `WCONTINUED` is
Linux/BSD-supported; huck's compat target is Linux (`ubuntu-24.04`) — no cfg guard needed (the code
already uses `WUNTRACED`/`WIFSTOPPED` unconditionally).

## Testing

Because bash emits async stop/continue **notifications** even non-interactively under `set -m`
(verified: it prints `[1]+ Stopped` before the next command with no `jobs` call at all — deferred
scope (b)), bash is not a clean byte-oracle for a full stop-sequence stream. The gate isolates
**state correctness**:

1. **Engine unit tests** (`jobs.rs` `#[cfg(test)]`, primary/authoritative):
   - a `WIFCONTINUED` raw status passed to `reap()` transitions a `Stopped` job → `Running`
     (and is a no-op on an already-`Running` job); construct the raw status with the existing
     test helpers (mirror `reap_with_stopped_status_transitions_job_to_stopped_state`, `jobs.rs:666`).
   - a stop followed by a continue returns the job to `Running`.
2. **Targeted bash-diff harness** `tests/scripts/job_stop_cont_diff_check.sh` (state-isolating):
   for each sequence — `kill -STOP %1` then `jobs -s` / `jobs -r`; then `kill -s CONT %1` then
   `jobs -r` / `jobs -s`; and `bg` of a stopped job — extract the queried job's **state token**
   (e.g. the `Running`/`Stopped` word for `%1`, via a `grep`/`awk` that takes the last matching
   line) and byte-compare huck vs bash. Both must agree (`Stopped`, then `Running`). This tolerates
   the async-notice *count* difference (scope b) while proving the state flips correctly. Use a
   deterministic settle (`sleep 1` after each signal) and `kill -9` cleanup; register in
   `run_diff_checks.sh` (auto-discovered by the `*_diff_check.sh` glob).
3. **Regression:** the engine lib suite stays green; no existing job-control diff-check regresses.

## Scope boundary / consequence

v299 fixes **state correctness** only. The full `jobs` bash-suite category **stays FAIL** — its
async `[1]+ Stopped`/`Running` notification lines still diff (deferred scope (b)) — this is
expected and noted in `docs/bash-test-suite-baseline.md`. Non-goals: async notifications (b);
job-spec gaps (c). Both remain tracked (open follow-ups off #158).
