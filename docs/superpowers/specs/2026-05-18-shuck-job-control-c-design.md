# shuck Job Control Design (Sub-project C: job specs, kill, disown)

**Status:** Approved 2026-05-18.
**Builds on:** [sub-project A](2026-05-16-shuck-job-control-design.md), [sub-project B](2026-05-18-shuck-job-control-b-design.md).

## Overview

Sub-projects A and B added background jobs (`&`, `jobs`, `wait`) and
foreground job control (`fg`, `bg`, Ctrl-Z). All those builtins are
no-arg only — they always operate on "the current job." This iteration
adds the **job-specifier** parser (`%N`, `%+`, `%%`, `%-`) and wires it
into every job-aware builtin, plus introduces the `kill` and `disown`
builtins. After this lands, shuck has a complete interactive job-control
surface modulo the explicitly-out-of-scope items in §8.

## Goals

- `%N`, `%+`, `%%`, `%-` job specifiers parse uniformly and resolve via
  the existing `JobTable`.
- `kill PID`, `kill %N`, `kill -<sig> ...` work. Sig names cover the
  common set (HUP, INT, QUIT, KILL, TERM, STOP, CONT, USR1, USR2) plus
  any numeric value.
- `disown` (no-arg or `%spec`) removes a job from the table without
  signaling it.
- `fg`, `bg`, `wait` accept `%spec`; `wait` also accepts a bare PID.

## Non-goals (deferred or out of scope entirely)

- `%cmd` and `%?cmd` prefix/substring specifiers.
- `wait -n` (wait for any one job).
- `kill -l`/`-s`/`-L` flag variants.
- `disown -a`/`-r`/`-h`.
- Backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`) — a separate
  feature requiring real subshell semantics.
- `set -m`/`+m` — shuck is always interactive.

## Architecture

One new module `src/job_spec.rs` owns parsing. `JobTable` gains a
single `resolve(&JobSpec) -> Option<u32>` method that wraps existing
`current_id` / id-lookup logic. Each job-aware builtin follows the same
pattern: peel a leading `%` argument, parse, resolve, fall back to a
no-arg behavior matching v7. `kill` and `disown` are new builtins
following the existing `is_builtin` + `run_builtin` dispatch pattern.

No changes to the lexer or parser — job specifiers are just argument
strings interpreted by the relevant builtin.

## 1. `JobSpec` parser and resolver

### New module `src/job_spec.rs`

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum JobSpec {
    Id(u32),       // %1, %2, ...
    Current,       // %+, %%
    Previous,      // %-
}

#[derive(Debug, PartialEq, Eq)]
pub enum JobSpecError {
    Empty,         // bare "%"
    BadNumber,     // "%abc", "%1x"
    BadSymbol,     // "%~", "%!", anything other than +, %, -
}

/// Parses a job-spec argument. The leading `%` is required.
pub fn parse_job_spec(s: &str) -> Result<JobSpec, JobSpecError>;
```

Parser table:

| Input       | Result                          |
| ----------- | ------------------------------- |
| `%`         | `Err(Empty)`                    |
| `%+`        | `Ok(Current)`                   |
| `%%`        | `Ok(Current)`                   |
| `%-`        | `Ok(Previous)`                  |
| `%<digits>` | `Ok(Id(n))` if parses to `u32`  |
| `%<other>`  | `Err(BadSymbol)` or `Err(BadNumber)` (latter only if starts with digit) |

The function does **not** accept inputs without a leading `%`. Builtins
check for `%` before calling it.

### `JobTable::resolve`

```rust
/// Resolves a JobSpec to a job id, if any matching job exists.
pub fn resolve(&self, spec: &JobSpec) -> Option<u32> {
    match spec {
        JobSpec::Id(id) => self.jobs.iter().find(|j| j.id == *id).map(|j| j.id),
        JobSpec::Current => self.current_id(),
        JobSpec::Previous => {
            let (_, prev) = self.current_and_previous();
            prev
        }
    }
}
```

Note: `current_and_previous` includes Done/Signaled jobs awaiting
notification, while `current_id` does not. `resolve(Previous)` uses
`current_and_previous` to match bash semantics — `%-` should resolve to
the previous job even if it just completed.

### Builtin call pattern

Each job-aware builtin uses this shape:

```rust
fn resolve_spec_or_error(
    arg: &str,
    builtin: &str,
    shell: &Shell,
) -> Result<u32, ExecOutcome> {
    let spec = parse_job_spec(arg).map_err(|_| {
        eprintln!("shuck: {builtin}: {arg}: bad job spec");
        ExecOutcome::Continue(1)
    })?;
    shell.jobs.resolve(&spec).ok_or_else(|| {
        eprintln!("shuck: {builtin}: {arg}: no such job");
        ExecOutcome::Continue(1)
    })
}
```

Builtins call `resolve_spec_or_error` when an argument starts with `%`.
This helper lives near `builtin_fg`/`builtin_bg` in `src/builtins.rs`
(it's an internal helper, not part of the `JobTable` API).

## 2. `kill` builtin

### Grammar

```
kill [-<sig>] <target> [<target>...]
```

- `<sig>` (optional) — signal name or number. If absent, defaults to `SIGTERM`.
- `<target>` — bare PID (positive integer) OR `%spec`.
- At least one target required.

### Signal table

```rust
fn signal_by_name(s: &str) -> Option<i32> {
    let upper = s.to_ascii_uppercase();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper);
    Some(match name {
        "HUP"  => libc::SIGHUP,
        "INT"  => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "KILL" => libc::SIGKILL,
        "TERM" => libc::SIGTERM,
        "STOP" => libc::SIGSTOP,
        "CONT" => libc::SIGCONT,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        _ => return None,
    })
}
```

Signal numbers parse via `<arg>.parse::<i32>()` and are accepted as long
as the value is plausible (1..=64). Out-of-range numbers fail with
`shuck: kill: <n>: invalid signal number`.

### Argument parsing

If `args[0]` starts with `-`:
- Strip the leading `-`. Parse the remainder as a signal (number or name). Failure → `shuck: kill: <arg>: invalid signal`, status 1.
- Targets are `args[1..]`. If empty, usage error.

Otherwise `sig = SIGTERM` and `targets = &args[..]`.

If no targets remain: `shuck: kill: usage: kill [-sig] pid | %job ...`, status 2.

### Per-target dispatch

For each `<target>`:
- If starts with `%`: resolve to job id via `resolve_spec_or_error`. Look up the job's `pgid`; call `libc::killpg(pgid, sig)`. On failure, capture `errno` and print `shuck: kill: ({target}) - {errstr}`.
- Otherwise: parse as `i32` (PID). If parse fails: `shuck: kill: {target}: arguments must be process or job IDs`, status 1, continue. Otherwise call `libc::kill(pid, sig)`. On failure, same error format.

### Exit status

- 0 if all targets succeeded.
- 1 if any target failed (printed individually).
- 2 if no targets supplied or signal parse failed.

### Rationale: pgrp targeting for `%spec`

Matches bash. Sending a signal to one stage of a pipeline (e.g., the
last) often misses the upstream stages. Targeting the pgrp delivers the
signal to every stage at once.

## 3. `disown` builtin

### Grammar

```
disown [%spec]
```

No args → operate on the current job (`current_id`). One `%spec` arg →
resolve it. Anything else → usage error, status 2.

### Semantics

```rust
fn builtin_disown(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("shuck: disown: usage: disown [%job]");
        return ExecOutcome::Continue(2);
    }
    let id = match args.first() {
        Some(a) if a.starts_with('%') => match resolve_spec_or_error(a, "disown", shell) {
            Ok(id) => id,
            Err(outcome) => return outcome,
        },
        Some(a) => {
            eprintln!("shuck: disown: usage: disown [%job]");
            let _ = a;
            return ExecOutcome::Continue(2);
        }
        None => match shell.jobs.current_id() {
            Some(id) => id,
            None => {
                eprintln!("shuck: disown: no current job");
                return ExecOutcome::Continue(1);
            }
        },
    };
    shell.jobs.jobs_mut().retain(|j| j.id != id);
    ExecOutcome::Continue(0)
}
```

The job is removed outright. The child process keeps running. The
SIGCHLD reaper's `JobTable::reap` is already a silent no-op for pids
that don't match any tracked job — when the disowned process eventually
exits, the kernel still reaps it via `waitpid(-1, ...)` and no zombie
remains.

Pending Done/Signaled notification is also dropped because the job is
gone before the next prompt's `drain_notifications` runs.

## 4. Extending `fg`/`bg`/`wait`

### `fg`

```
fg [%spec]
```

- No args: existing v7 behavior (current job via `current_id`).
- One `%spec`: resolve, then run the existing fg logic on the resolved id.
- Otherwise: `shuck: fg: usage: fg [%job]`, status 2.
- If the resolved job is `Done` or `Signaled` (already-completed race): `shuck: fg: <arg>: no such job`, status 1.

### `bg`

```
bg [%spec]
```

- No args: v7 behavior (current Stopped job via `current_stopped_id`).
- One `%spec`: resolve, then verify state is `Stopped(_)`. If not: `shuck: bg: job %<id> already running`, status 1.
- Otherwise: usage error, status 2.

### `wait`

```
wait [%spec | PID]
```

- No args: v7 behavior (poll until `!has_pending()`).
- One `%spec`: resolve to id, then poll until that specific job leaves Running/Stopped. Return its decoded exit status (`Done(c)` → `c`, `Signaled(s)` → `128+s`, missing → 127).
- One bare integer: parse as `i32` PID. Call `libc::waitpid(pid, _, WNOHANG | WUNTRACED)` in the same poll loop; on `r > 0`, also feed it to `shell.jobs.reap` (so any matching job's state stays consistent). Return decoded exit status. If `waitpid` returns -1 with ECHILD on the very first call, `shuck: wait: pid <n> is not a child of this shell`, status 127.
- Otherwise: usage error, status 2.
- SIGINT handling: identical to v7's wait — `compare_exchange(true, false)` on `shell.sigint_flag` each iteration, return 130 if set.

## 5. Edge cases & errors

| Scenario                                          | Behavior                                                          |
| ------------------------------------------------- | ----------------------------------------------------------------- |
| `%abc`                                            | `shuck: <builtin>: %abc: bad job spec`, status 1                  |
| `%9999` (no such job)                             | `shuck: <builtin>: %9999: no such job`, status 1                  |
| `%-` when only one job exists                     | `shuck: <builtin>: %-: no such job`, status 1                     |
| `kill -ABC <target>`                              | `shuck: kill: ABC: invalid signal`, status 1                      |
| `kill -99 <target>`                               | `shuck: kill: 99: invalid signal number`, status 1                |
| `kill <bad-pid>`                                  | `shuck: kill: <arg>: arguments must be process or job IDs`, status 1 |
| `kill 99999` (no such process)                    | `shuck: kill: (99999) - No such process`, status 1                |
| `disown %1; jobs`                                 | `jobs` lists nothing (or remaining jobs)                          |
| `wait 99999` (not a child)                        | `shuck: wait: pid 99999 is not a child of this shell`, status 127 |
| `bg %1` when job 1 is Running                     | `shuck: bg: job %1 already running`, status 1                     |
| `fg %1` when job 1 was just reaped to Done        | `shuck: fg: %1: no such job`, status 1                            |
| `kill` (no args)                                  | usage, status 2                                                   |
| `kill -TERM` (no targets)                         | usage, status 2                                                   |

## 6. Testing

Unit-testable (deterministic, no real subprocesses needed):

- `parse_job_spec` table: `%`, `%+`, `%%`, `%-`, `%1`, `%99`, `%abc`, `%1x`, `%~`, `%-1`. Cover both Ok and Err arms.
- `JobTable::resolve` for each variant on a synthetic table with mixed states.
- `signal_by_name` table: `TERM`, `SIGTERM`, `term`, `sigterm`, `ABC` (None), empty string.
- `builtin_kill` argument routing: no args, no targets after sig, invalid signal, invalid target form. (Actual signal delivery to real processes is integration-level — skip.)
- `builtin_disown`: no current job, valid `%N`, table is empty afterwards, suppresses pending notification (use a synthetic Done job with `notified=false` and confirm it's gone after disown).
- `builtin_fg`/`builtin_bg`/`builtin_wait` arg validation: usage errors, no such job, `bg` on Running job, `wait` on bare PID parse failure.

Manual smoke (real tty, post-merge):

1. `sleep 100 &`, `sleep 200 &`, `jobs` → two running. `kill %1` → job 1 goes Killed (signal 15). `jobs` → only job 2.
2. `sleep 100 &; kill -STOP %1; jobs` → `[1]+ Stopped              sleep 100`.
3. `bg %1` → continues. `jobs` → Running.
4. `sleep 100 &; disown %1; jobs` → empty. `ps` (in another terminal) shows sleep still running.
5. `sleep 5 &; wait %1; echo $?` → after ~5s, prints `0`.
6. `sleep 5 &; wait <pid-of-sleep>; echo $?` → after ~5s, prints `0`.
7. `wait 99999` → immediate `not a child` error, status 127.
8. `sleep 1000 &; sleep 1000 &; disown %1; wait` → blocks on job 2 only.

## 7. File summary

| File              | Changes                                                                                                |
| ----------------- | ------------------------------------------------------------------------------------------------------ |
| `src/job_spec.rs` | NEW — `JobSpec`, `JobSpecError`, `parse_job_spec`.                                                     |
| `src/main.rs`     | `mod job_spec;`.                                                                                       |
| `src/jobs.rs`     | `JobTable::resolve(&JobSpec) -> Option<u32>`.                                                          |
| `src/builtins.rs` | `is_builtin` adds `kill`, `disown`. `run_builtin` dispatch for both. `builtin_kill`, `builtin_disown`. Extend `builtin_fg`, `builtin_bg`, `builtin_wait` to accept job specs / PIDs. Internal helper `resolve_spec_or_error`. |

No new dependencies. `libc` already provides `kill`, `killpg`, `waitpid`, all needed signal constants.

## 8. Out of scope / future work

- `%cmd` / `%?cmd` job-spec forms.
- `wait -n`.
- `kill -l` / `-s` / `-L`.
- `disown -a` / `-r` / `-h`.
- `set -m` / `+m`.
- Backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`) — separate feature.
- `time` / `times` builtins for process accounting.
