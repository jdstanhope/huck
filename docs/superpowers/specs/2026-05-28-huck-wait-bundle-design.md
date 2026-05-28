# huck v40 — `wait -n` + multi-arg `wait` (M-37 + M-38)

## Goal

Close two related bash divergences in huck's `wait` builtin:

- **M-37**: `wait -n` — wait for the *next* job/PID to finish out of either
  all running jobs or an explicit subset, including the bash 5.1 `-p VAR`
  flag that captures the finished job's PID.
- **M-38**: `wait PID1 PID2 …` / `wait %1 %2` — wait for *all* listed jobs
  to finish; exit status = status of the last one waited.

Both share the same builtin so they ship together as v40.

After v40, the bash `wait` surface huck supports is:

```
wait [-n] [-p VAR] [id ...]
```

Where each `id` is either a `%job-spec` or a positive integer PID. `-p` is
only valid in combination with `-n` (bash 5.1 rule).

## Scope decisions (locked)

1. **Multi-arg status (M-38)**: bash-faithful — wait for all listed targets
   sequentially; exit status is that of the *last* one waited.
2. **`wait -n` with no running jobs (M-37)**: bash-faithful — return 127
   immediately.
3. **`wait -n` with an explicit target list (M-37)**: include — `wait -n %1
   1234` waits for whichever of those finishes first.
4. **`-p VAR` (bash 5.1)**: include — captures the finished job's PID
   (`pgid` for job-spec targets, literal PID for PID targets) into `$VAR`.

## Out of scope (explicitly deferred)

- `wait -f` (bash 5.1 "wait for state change including stopped"). M-* item
  for a later iteration.
- `wait -np var` combined-flag form. Easy to add later if asked.
- Argument-form validation that detects `wait %1 -n` (flags AFTER
  positionals). v40's parser stops flag scanning at the first non-flag
  token; trailing `-n` becomes a positional and fails as "not a pid or
  valid job spec" — acceptable.

## Architecture

All changes confined to `src/builtins.rs`. No new files, no new modules.

The existing `builtin_wait` (currently `src/builtins.rs:342-364`) is
replaced with a small dispatcher that:

1. Parses flags and positional args into a `WaitArgs` struct.
2. Validates the `-p`-requires-`-n` constraint.
3. Selects one of five paths based on `(wait_any, targets.len())`.

The three existing helpers (`wait_all`, `wait_for_job`, `wait_for_pid`)
are reused as-is. Three new helpers handle the new paths.

### New internal types

```rust
enum WaitTarget {
    Job(u32),  // resolved from a %spec
    Pid(i32),  // a positive integer PID arg
}

struct WaitArgs {
    wait_any: bool,
    pid_var: Option<String>,
    targets: Vec<WaitTarget>,
}
```

### New parser

```rust
fn parse_wait_args(args: &[String], shell: &Shell) -> Result<WaitArgs, ExecOutcome>
```

Flag pass: walk `args` from index 0. Recognized flags:

- `-n` → set `wait_any = true`, advance one.
- `-p` → require the next token; capture as `pid_var`, advance two. If
  there is no next token: error 2 with message
  `huck: wait: -p: option requires a variable name`.
- `--` → end of flags, advance one.
- Any other arg starting with `-` and longer than 1 char: error 2 with
  message `huck: wait: <arg>: invalid option` followed by usage line.
- First non-flag token: stop flag pass.

After the flag pass: validate `pid_var.is_some()` implies `wait_any`. If
violated, error 2 with message `huck: wait: -p: option requires -n`.

Positional pass: walk the remainder. For each token:

- Starts with `%`: pass to existing `resolve_spec_or_error` (already
  returns the right error/status). On success push `WaitTarget::Job(id)`.
- Otherwise: parse as `i32`. If parse succeeds and value > 0 push
  `WaitTarget::Pid(v)`. Otherwise error 2 with message
  `huck: wait: <arg>: not a pid or valid job spec`.

The parser does NOT block on waitpid — it only validates. Multi-arg
errors must surface *before* any waiting begins (per scope decision 1).

### Dispatch

```rust
match (args.wait_any, args.targets.len()) {
    (false, 0) => wait_all(shell),
    (false, 1) => match &args.targets[0] {
        WaitTarget::Job(id) => wait_for_job(*id, shell),
        WaitTarget::Pid(pid) => wait_for_pid(*pid, shell),
    },
    (false, _) => wait_for_all(args.targets, shell),
    (true, 0) => wait_any_pending(args.pid_var, shell),
    (true, _) => wait_any_of(args.targets, args.pid_var, shell),
}
```

### New helper: `wait_for_all`

```rust
fn wait_for_all(targets: Vec<WaitTarget>, shell: &mut Shell) -> ExecOutcome
```

Sequentially waits for each target. After each wait, capture the
returned status code. Return the *last* status (per scope decision 1). If
any per-target wait returns a non-Continue outcome (e.g. SIGINT 130),
propagate immediately.

```rust
let mut last = 0;
for t in targets {
    let outcome = match t {
        WaitTarget::Job(id) => wait_for_job(id, shell),
        WaitTarget::Pid(pid) => wait_for_pid(pid, shell),
    };
    match outcome {
        ExecOutcome::Continue(c) => last = c,
        other => return other,
    }
}
ExecOutcome::Continue(last)
```

### New helper: `wait_any_pending`

```rust
fn wait_any_pending(pid_var: Option<String>, shell: &mut Shell) -> ExecOutcome
```

Behavior:

1. Snapshot the set of currently-pending job ids at entry. "Pending"
   here means `JobState::Running` — Stopped jobs are excluded (matches
   bash 5.x `-n` semantics).
2. If the snapshot is empty: if `pid_var` is set, assign `$VAR = ""`.
   Return `Continue(127)`.
3. Otherwise enter the poll loop (mirrors `wait_all`):
   - Check SIGINT → return 130.
   - `waitpid(-1, …, WNOHANG | WUNTRACED)`; on positive `r`, call
     `shell.jobs.reap(r, status)`.
   - After reaping (or after the 50 ms sleep), iterate the snapshot:
     find any job whose state is now `Done(c)` or `Signaled(s)`.
4. On first match: extract `(pgid, status)` from the job, set
   `$VAR = pgid.to_string()` if `pid_var`, return `Continue(status)`.
5. If all snapshotted jobs disappear from the table without transitioning
   to terminal (e.g. external `disown`): treat as the empty-jobs case and
   return 127.

`status` for `Done(c)` is `c`; for `Signaled(s)` is `128 + s` (matches
existing helpers).

### New helper: `wait_any_of`

```rust
fn wait_any_of(targets: Vec<WaitTarget>, pid_var: Option<String>, shell: &mut Shell) -> ExecOutcome
```

Same poll loop as `wait_any_pending`, but the "is anyone finished?"
check filters to the provided targets:

- For each `WaitTarget::Job(id)`: look up the job by id in
  `shell.jobs.iter()`. If state is `Done(c)` or `Signaled(s)`, capture
  `(job.pgid, status)`. Per the spec, `-p` records the **pgid** for
  job-spec targets.
- For each `WaitTarget::Pid(pid)`: call
  `waitpid(pid, &mut status, WNOHANG | WUNTRACED)`. If it returns the
  PID (positive), the process has finished — feed the raw status to
  `shell.jobs.reap(pid, status)` to keep the job table consistent, then
  capture `(pid, decoded_status)`. If it returns 0 the process is still
  alive. If it returns -1 with ECHILD: not (or no longer) a child —
  treat as "this target can never finish from our side" and surface as
  127 only if NO other target is active either.

Pre-check at entry (before the poll loop) handles already-terminal
targets. If any target is already terminal at entry, return it
immediately. The pre-check uses the same logic as one poll iteration
(no SIGCHLD/sleep).

If at entry every target resolves to "not a child / no such running
job" (i.e. none of them can ever finish): return 127 with
`$VAR = ""`.

If no target is currently active (e.g. `wait -n 9999` where 9999 is not
a known child): return 127 immediately. `pid_var` set to `""` in that
case.

### Error message format

All error messages prefix with `huck: wait:` to match existing builtin
convention (see `wait_for_pid` at `src/builtins.rs:434`). Status code 2
for usage errors (matches the existing usage-error branch).

| Condition | Message | Status |
|---|---|---|
| `-p` without `-n` | `huck: wait: -p: option requires -n` | 2 |
| `-p` with no following token | `huck: wait: -p: option requires a variable name` | 2 |
| Unknown flag `-X` | `huck: wait: -X: invalid option\nhuck: wait: usage: wait [-n] [-p var] [id ...]` | 2 |
| Bad PID/spec | `huck: wait: <arg>: not a pid or valid job spec` | 2 |
| Bad `%spec` lookup | (existing) `huck: wait: <arg>: bad job spec` or `huck: wait: <arg>: no such job` | 1 |

The pre-existing "no such job" / "bad job spec" path (status 1) is
intentionally NOT changed; those go through `resolve_spec_or_error` and
the messages and codes there remain stable.

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod tests`

~10 tests, all using the existing `run_builtin` test harness that the
`wait_with_*` tests already use:

1. `wait_multiarg_returns_last_status` — both args are already-Done jobs
   in the table; verify return code matches the second arg's status.
2. `wait_multiarg_unparseable_arg_errors_status_2` — `wait 1234 abc` →
   status 2, no waitpid call.
3. `wait_multiarg_mixed_pid_and_spec` — `wait %1 %2` where both
   pre-loaded Done; returns last status.
4. `wait_n_with_no_jobs_returns_127` — empty job table → status 127
   immediately.
5. `wait_n_returns_status_of_finished_job` — pre-load one Done(7) job;
   `wait -n` returns 7.
6. `wait_n_with_already_done_target_returns_immediately` — pre-load Done
   %1; `wait -n %1` returns its status without polling.
7. `wait_n_p_var_captures_pgid` — pre-load Done %1 with pgid 12345;
   `wait -n -p PID` returns its status AND `shell.lookup_var("PID")`
   equals `"12345"`.
8. `wait_p_without_n_is_usage_error` — `wait -p VAR` → status 2 with
   `-p: option requires -n` on stderr.
9. `wait_n_p_without_var_name_is_usage_error` — `wait -n -p` (no var) →
   status 2.
10. `wait_invalid_flag_is_usage_error` — `wait -x` → status 2 with
    `invalid option`.

The existing `wait_with_multiple_args_returns_usage_status_2` test must
be repurposed or replaced — its assumption (multi-arg = usage error)
is no longer true. Rename to
`wait_multiarg_unparseable_returns_usage_status_2` and adjust the input
to include a bad arg.

### Integration tests at `tests/wait_integration.rs`

New file mirroring `tests/arith_completion_integration.rs` style. ~5
tests using the existing `run(script)` harness:

1. `wait_n_returns_status_of_first_finished` — `(sleep 0.05; exit 7) &
   wait -n` → stdout-from-`echo $?` is `7`.
2. `wait_multiarg_all_succeed` — `(true) & (true) & wait %1 %2; echo $?`
   → stdout `0`.
3. `wait_multiarg_returns_last_status` — `(exit 5) & (exit 3) & wait %1
   %2; echo $?` → stdout `3` (second job's exit code).
4. `wait_n_no_jobs_returns_127` — `wait -n; echo $?` → stdout `127`.
5. `wait_n_p_captures_pid` — `(sleep 0.05; exit 3) & wait -n -p FINPID;
   echo $FINPID; echo $?` → first line is the bg job's PID (an integer),
   second line is `3`.

The PID-capture integration test cannot easily assert the exact PID
value (it varies per run). Assertion: parses to a positive integer.

### Smoke

Full suite (`cargo test --all-targets`) must pass after the change. PTY
flake `pty_compound_stage_pipeline_stops_and_resumes` continues to be
tolerated per prior iterations.

## Implementation tasks

1. **Builtin core**: rewrite `builtin_wait` dispatcher + add
   `parse_wait_args` + `WaitTarget`/`WaitArgs` + the three new helpers
   (`wait_for_all`, `wait_any_pending`, `wait_any_of`). Adjust the one
   existing unit test that asserted multi-arg was a usage error. Add the
   10 new unit tests.
2. **Integration tests**: create `tests/wait_integration.rs` with the 5
   tests above.
3. **Docs**: flip M-37 and M-38 to `[fixed v40]`; README v40 row;
   change-log entry. No new L-* divergences (the behavior matches bash).
   Full-suite verify.

Three tasks. TDD within each, one commit per task.

## Acceptance criteria

- All new unit tests pass.
- All new integration tests pass.
- The existing `wait_with_multiple_args_returns_usage_status_2` test is
  either renamed/adjusted to reflect the new semantics, or removed if no
  longer meaningful (recommended: rename to
  `wait_multiarg_unparseable_returns_usage_status_2`).
- `cargo test --all-targets` passes (modulo known PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` shows M-37 and M-38 as `[fixed v40]`.
- `wait -n -p VAR` correctly captures the finished job's PID into the
  named shell variable.
- Multi-arg `wait` returns the exit status of the last waited target.
