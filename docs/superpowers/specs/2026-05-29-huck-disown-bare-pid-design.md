# huck v44 — `disown` accepts bare PID (M-44)

## Goal

Close bash divergence M-44: `disown` should accept a bare PID
(`disown 12345`) in addition to the `%spec` form. The PID matches
any pid in any tracked job's `pids` list (including non-leader
pipeline stages); the operation acts on the entire job.

## Scope decisions (locked)

This is a small one-arm change. No scope questions surfaced
worth asking; the recommended bash-faithful semantics are
unambiguous:

1. **Match scope**: match against any `pid` in any job's `pids`
   vec, not just the pgid. Operates on the whole job once a
   match is found.
2. **Unknown PID**: error with `huck: disown: <arg>: no such
   job` + status 1.
3. **Unparseable / non-positive**: error with `huck: disown:
   <arg>: not a valid job spec` + status 1 (preserves existing
   wording for non-numeric args).
4. **Coexistence with flags**: `disown -h 12345` works (flag
   parser eats `-h`, positional `12345` resolves via the new
   path). `disown -a 12345` ignores `12345` (bash-faithful `-a`
   behavior from v43).

## Out of scope (deferred)

- Negative PID forms (`disown -123`). These get consumed by the
  flag parser before reaching the positional loop, errors there
  with "invalid option" + status 2. Not changing.
- PID forms in other builtins (`fg`, `bg`, `kill -s` etc).
  `kill` already accepts bare PIDs via a different code path.
  `fg`/`bg` continue to require `%spec`; that's a separate
  divergence not tracked under M-44.

## Architecture

Single-file change in `src/builtins.rs`. The current
`builtin_disown` positional loop
(`src/builtins.rs:995-1004`) is:

```rust
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
```

Replace with:

```rust
for arg in positional {
    if arg.starts_with('%') {
        match resolve_spec_or_error(arg, "disown", shell) {
            Ok(id) => ids.push(id),
            Err(outcome) => return outcome,
        }
    } else {
        match arg.parse::<i32>() {
            Ok(pid) if pid > 0 => {
                match shell.jobs.iter().find(|j| j.pids.contains(&pid)) {
                    Some(job) => ids.push(job.id),
                    None => {
                        eprintln!("huck: disown: {arg}: no such job");
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            _ => {
                eprintln!("huck: disown: {arg}: not a valid job spec");
                return ExecOutcome::Continue(1);
            }
        }
    }
}
```

The remainder of `builtin_disown` (flag parser, `-a`/`-r`/`-h`
job-set selection, `running_only` retain filter, mark-or-remove
action) is unchanged.

### Error message table

| Condition | Message | Status |
|---|---|---|
| `disown 12345` (PID not in any job) | `huck: disown: 12345: no such job` | 1 |
| `disown abc` | `huck: disown: abc: not a valid job spec` | 1 |
| `disown 0` | `huck: disown: 0: not a valid job spec` | 1 |
| `disown 12345` (valid PID match) | (no output) | 0 |

Negative integers like `disown -123` never reach the positional
loop — the flag parser at the top of `builtin_disown` consumes
the leading `-` and errors with `huck: disown: -1: invalid
option` at status 2. The new positional arm only sees args that
either start with `%` or don't start with `-`.

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod disown_tests`

4 new tests:

1. `disown_bare_pid_matches_job_leader`
   - Pre-load `shell.jobs.add(1234, vec![1234], "sleep".to_string())`.
   - `run_builtin("disown", &["1234"], ...)`.
   - Expect `Continue(0)` and the job removed from the table.

2. `disown_bare_pid_matches_pipeline_stage`
   - Pre-load `shell.jobs.add(1234, vec![1234, 1235, 1236], "a | b | c".to_string())`.
   - `run_builtin("disown", &["1235"], ...)`.
   - Expect `Continue(0)` and the whole job removed (verifies
     match against non-leader pid).

3. `disown_unknown_pid_errors_status_1`
   - Empty job table.
   - `run_builtin("disown", &["99999"], ...)`.
   - Expect `Continue(1)`.

4. `disown_h_with_bare_pid_marks_job`
   - Pre-load `shell.jobs.add(1234, vec![1234], "sleep".to_string())`.
   - `run_builtin("disown", &["-h", "1234"], ...)`.
   - Expect `Continue(0)`, job stays in table,
     `marked_for_nohup == true`.

### Integration test at `tests/disown_pid_integration.rs`

One test:

1. `disown_h_with_bare_pid_lets_bg_survive`
   - Script: `sleep 30 >/dev/null 2>&1 &\necho $!\ndisown -h $!\nexit\n`.
   - Capture PID from stdout via the same `first_pid` helper
     used in v43.
   - Wait 200ms after huck exits.
   - Assert `libc::kill(pid, 0) == 0` (still alive).
   - SIGTERM cleanup.

Reuses the harness pattern from `tests/disown_h_integration.rs`.
The test verifies the PID-resolution path works end-to-end via
the binary, not just at unit level.

### Smoke

`cargo test --all-targets` must pass after the change. PTY
flake tolerated.

## Implementation tasks

1. **Builtin change + unit tests**: rewrite the positional
   loop arm in `builtin_disown`; append 4 new tests.
2. **Integration test**: create `tests/disown_pid_integration.rs`
   with the one scenario above.
3. **Docs**: flip M-44 to `[fixed v44]` in
   `docs/bash-divergences.md`; change-log entry; README v44 row.

Three tasks. TDD per task; commit per task.

## Acceptance criteria

- All 4 new unit tests pass.
- The integration test passes.
- `cargo test --all-targets` passes (modulo known PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` shows M-44 as `[fixed v44]`.
- `disown 12345` works on a tracked bg PID; `disown -h 12345`
  marks it; `disown UNKNOWN_PID` errors with "no such job".
- All v43 disown tests still pass (no regression on `%spec`
  path or flag-set behavior).
