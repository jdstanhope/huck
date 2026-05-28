# huck v43 — `disown -a/-r/-h` + SIGHUP-on-exit (M-43)

## Goal

Close bash divergence M-43 by adding flag support and multi-arg
support to `disown`, plus the SIGHUP-on-exit behavior that gives `-h`
real semantics.

After v43 the `disown` surface is:

```
disown [-ahr] [%job ...]
```

## Scope decisions (locked)

1. **Full `-h` semantics**: implement actual SIGHUP-on-exit so `-h`
   has observable effect. New `marked_for_nohup: bool` field on
   `Job`; new `Shell::hangup_jobs()` method called from the
   shell-exit path.
2. **Multi-arg support**: `disown %1 %2 %3` valid. Per-arg errors
   continue processing (mirrors bash).

## Behavior change

**Before v43**: background jobs survive a clean `huck` exit (huck
never sent SIGHUP).

**After v43**: background jobs receive SIGHUP on clean exit (matching
bash's typical interactive default), unless marked with `disown -h`.
Existing defensive patterns (`disown -h`, `nohup ...`) continue to
work; scripts that relied on huck's old "always survives exit"
behavior need to add `disown -h $pid` (or `disown -ah`).

Documented as a behavior change in the change-log entry. NOT a new
L-* divergence — bash does the same thing.

## Out of scope (deferred)

- `shopt -s huponexit` / `shopt -u huponexit` to gate the SIGHUP-on-exit
  behavior globally. Future iteration. For v43, SIGHUP-on-exit is
  always-on; `-h` is the only opt-out.
- Bash 5.x `disown -p` (print pid) and other rare extensions.
- Interaction with subshells. Subshells don't reach the
  `hangup_jobs` call (they exit via a different path); their jobs
  inherit `marked_for_nohup` via fork but `Shell::hangup_jobs` is not
  invoked there.

## Architecture

Changes span three files:

1. **`src/jobs.rs`** — add `marked_for_nohup: bool` field to `Job`
   and `mark_for_nohup(id)` helper on `JobTable`.
2. **`src/shell_state.rs`** — add `Shell::hangup_jobs(&mut self)`
   method.
3. **`src/builtins.rs`** — rewrite `builtin_disown` to parse flags
   and multi-arg.
4. **`src/shell.rs`** OR `src/main.rs` — call `shell.hangup_jobs()`
   at the natural exit point.

### `Job::marked_for_nohup`

Add the field at the end of the existing `Job` struct
(`src/jobs.rs:19-29`). Initialize to `false` in `JobTable::add` and
`JobTable::add_synthetic_done`.

```rust
pub struct Job {
    pub id: u32,
    pub pgid: i32,
    pub pids: Vec<i32>,
    pub reaped: Vec<bool>,
    pub last_status: Option<i32>,
    pub command: String,
    pub state: JobState,
    pub notified: bool,
    pub created_at: u64,
    pub marked_for_nohup: bool,  // NEW
}
```

### `JobTable::mark_for_nohup`

```rust
pub fn mark_for_nohup(&mut self, id: u32) {
    if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
        job.marked_for_nohup = true;
    }
}
```

### `Shell::hangup_jobs`

Add a public method on `Shell` (in `src/shell_state.rs`, alongside
existing methods like `set`/`lookup_var`):

```rust
/// Sends SIGHUP to every live job not marked for nohup. Called at
/// shell exit. Stopped jobs get SIGCONT first so they wake to die.
pub fn hangup_jobs(&mut self) {
    for job in self.jobs.iter() {
        if !should_hangup(job) {
            continue;
        }
        unsafe {
            libc::killpg(job.pgid, libc::SIGCONT);
            libc::killpg(job.pgid, libc::SIGHUP);
        }
    }
}

/// Pure predicate isolating the hangup decision for testability.
fn should_hangup(job: &crate::jobs::Job) -> bool {
    let live = matches!(
        job.state,
        crate::jobs::JobState::Running | crate::jobs::JobState::Stopped(_)
    );
    live && !job.marked_for_nohup
}
```

### Hook site

Find the REPL exit point — likely in `src/shell.rs` where the main
loop returns after `ExecOutcome::Exit`. Insert a single call to
`shell.hangup_jobs()` immediately before the function returns. If the
exit path is in `src/main.rs` after the REPL function returns, place
it there instead. Implementer determines exact location during
Task 1.

If there are MULTIPLE exit paths (interactive Ctrl-D vs explicit
`exit` builtin vs signal-triggered termination), call `hangup_jobs`
in all of them — the safest pattern is to make `hangup_jobs`
idempotent (sending SIGHUP twice is harmless since waitpid drains
those signals and dead pgids return ESRCH which we ignore).

### Rewritten `builtin_disown`

Replace the existing `builtin_disown` body
(`src/builtins.rs:956-980`) with:

```rust
fn builtin_disown(args: &[String], shell: &mut Shell) -> ExecOutcome {
    // Flag parse: combined forms accepted (e.g. -ah, -arh).
    let mut all = false;
    let mut running_only = false;
    let mut mark_nohup = false;
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        if let Some(rest) = a.strip_prefix('-') {
            if rest.is_empty() {
                break; // bare `-` is positional
            }
            for c in rest.chars() {
                match c {
                    'a' => all = true,
                    'r' => running_only = true,
                    'h' => mark_nohup = true,
                    _ => {
                        eprintln!("huck: disown: -{c}: invalid option");
                        eprintln!("huck: disown: usage: disown [-ahr] [%job ...]");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            idx += 1;
        } else {
            break;
        }
    }

    let positional = &args[idx..];

    // Job-set selection.
    let mut target_ids: Vec<u32> = if all {
        // `-a` ignores positional args per bash.
        shell.jobs.iter().map(|j| j.id).collect()
    } else if !positional.is_empty() {
        let mut ids = Vec::new();
        for arg in positional {
            if !arg.starts_with('%') {
                eprintln!("huck: disown: {arg}: not a valid job spec");
                return ExecOutcome::Continue(1);
            }
            match resolve_spec_or_error(arg, "disown", shell) {
                Ok(id) => ids.push(id),
                Err(outcome) => return outcome,
            }
        }
        ids
    } else {
        match shell.jobs.current_id() {
            Some(id) => vec![id],
            None => {
                eprintln!("huck: disown: no current job");
                return ExecOutcome::Continue(1);
            }
        }
    };

    // -r filter
    if running_only {
        target_ids.retain(|id| {
            shell
                .jobs
                .iter()
                .find(|j| j.id == *id)
                .map(|j| matches!(j.state, crate::jobs::JobState::Running))
                .unwrap_or(false)
        });
    }

    // Per-job action.
    if mark_nohup {
        for id in &target_ids {
            shell.jobs.mark_for_nohup(*id);
        }
    } else {
        shell.jobs.jobs_mut().retain(|j| !target_ids.contains(&j.id));
    }

    ExecOutcome::Continue(0)
}
```

### Error message table

| Condition | Message | Status |
|---|---|---|
| `disown -x` (unknown flag) | `huck: disown: -x: invalid option` + usage | 2 |
| `disown` with no jobs | `huck: disown: no current job` | 1 |
| `disown %99` (no such job) | (existing) `huck: disown: %99: no such job` | 1 |
| `disown foo` (no `%`) | `huck: disown: foo: not a valid job spec` | 1 |
| `disown -r` no Running jobs | (no-op) | 0 |
| `disown -a` empty table | (no-op) | 0 |

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod disown_tests`

9 builtin tests + 1 should_hangup test:

1. `disown_a_removes_all_jobs` — pre-load 3 synthetic jobs;
   `disown -a` → table is empty afterward.
2. `disown_r_filters_to_running_only` — pre-load 1 Running + 2 Done
   jobs; `disown -r` → only the Running one removed.
3. `disown_h_marks_for_nohup_keeps_in_table` — pre-load Running %1;
   `disown -h %1` → job remains, `marked_for_nohup` is true.
4. `disown_multiple_args_processes_each` — pre-load 3 Done jobs;
   `disown %1 %2` → only %3 remains.
5. `disown_ah_marks_all` — pre-load 2 Running jobs; `disown -ah` →
   both stay, both `marked_for_nohup`.
6. `disown_ar_removes_all_running` — pre-load 2 Running + 1 Done;
   `disown -ar` → only the Done remains.
7. `disown_arh_marks_all_running` — pre-load 2 Running + 1 Done;
   `disown -arh` → all stay, only the 2 Running marked for nohup.
8. `disown_invalid_flag_returns_usage_status_2` — `disown -x` → 2.
9. `disown_a_ignores_positional_args` — pre-load 3 Running;
   `disown -a %1` → all 3 removed (bash-faithful: positional
   ignored when -a).

Plus the pure-function test:

10. `should_hangup_skips_marked_jobs` — directly drive the
    `should_hangup` helper with manually-constructed Job values.

### Integration tests at `tests/disown_h_integration.rs`

3 binary-driven tests. These use a Rust `Command` to launch huck,
script in `(sleep 30 & ; ...) ; exit`, capture the bg PID from
huck's `jobs -p` output, wait briefly after huck exits, then probe
the PID's liveness via `libc::kill(pid, 0)` (returns 0 if alive,
ESRCH if dead).

1. `disown_h_lets_bg_job_survive` — start `sleep 30`, capture PID,
   `disown -h %1`, exit; assert PID still alive after 200ms.
   Cleanup: send SIGTERM to the surviving sleep.
2. `disown_without_h_kills_bg_job_on_exit` — start `sleep 30`,
   capture PID, exit; assert PID DEAD after 200ms.
3. `disown_a_h_marks_all_alive` — start 2 `sleep 30` jobs,
   `disown -ah`, exit; assert BOTH PIDs alive. Cleanup: SIGTERM
   both.

Helper in the integration file:

```rust
fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}
```

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Job + Shell core**:
   - Add `marked_for_nohup: bool` to `Job` (default false in `add` /
     `add_synthetic_done`).
   - Add `JobTable::mark_for_nohup(id)`.
   - Add `Shell::hangup_jobs(&mut self)` and the pure
     `should_hangup(job)` helper.
   - Find the REPL exit path and call `shell.hangup_jobs()`.
2. **`builtin_disown` rewrite** with the parser + 9 unit tests +
   the should_hangup test. Replace the existing one
   `disown_*` test that asserted single-arg semantics if needed.
3. **Integration tests**: create `tests/disown_h_integration.rs`
   with the 3 scenarios.
4. **Docs**: flip M-43 to `[fixed v43]`, change-log entry, README
   v43 row.

Three tasks following the v40-v42 cadence: foundation + builtin
rewrite + 10 unit tests as a single task (the Job/Shell core and
the consumer builtin are tightly coupled), then integration tests,
then docs. TDD within each.

## Acceptance criteria

- All 10 unit tests pass.
- All 3 integration tests pass.
- `cargo test --all-targets` passes (modulo known PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` shows M-43 as `[fixed v43]`.
- `disown -h $PID` survives shell exit (verified by integration
  test).
- `disown -ar` removes all running jobs.
- Existing `disown %spec` and bare `disown` paths unchanged for
  default-current-job semantics.
- The behavior change (huck now sends SIGHUP on exit) is documented
  in the change-log entry.
