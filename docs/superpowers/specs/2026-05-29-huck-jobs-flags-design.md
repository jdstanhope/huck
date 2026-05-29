# huck v45 — `jobs -l/-p/-n/-r/-s` + positional `%spec` (M-45)

## Goal

Close bash divergence M-45 by adding the five bash `jobs` flags and
positional `%spec` filtering to huck's `jobs` builtin. After v45 the
surface is:

```
jobs [-lpnrs] [%spec ...]
```

## Scope decisions (locked)

1. **`-l` PID format for pipelines**: bash-faithful multi-line. First
   stage on the `[N]<flag> <pid>` line; subsequent stages on
   indented `     <pid>` lines.
2. **`-n` state-change tracking**: reuse the existing
   `Job.notified: bool` field. After `jobs -n` prints, set
   `notified = true` for each printed job. A regular `jobs` (no
   `-n`) does NOT consume the flag.
3. **Positional `%spec` filtering**: include. `jobs -l %1` filters
   to the named jobs only. Resolved via existing
   `resolve_spec_or_error`.

## Out of scope (deferred)

- Extended job specs (`jobs %cmd` / `jobs %?cmd`) — separate
  divergence (M-47 or similar).
- Byte-precise bash column-width alignment for `-l` multi-line output.
  Structure matches; exact widths may differ slightly.
- `jobs -x ...` (xargs-style command launcher) — bash extension out
  of scope for v45.

## Architecture

Two-file change:

- `src/jobs.rs`: new `notification_line_long(job, flag) -> Vec<String>`
  helper and new `JobTable::mark_notified(&[u32])` method.
- `src/builtins.rs`: rewrite `builtin_jobs` to parse flags + positional
  args and route through one of three output paths (default,
  `-l`, `-p`).

### New `notification_line_long`

```rust
/// Bash-faithful `jobs -l` output for a single job. Returns one
/// String per pipeline stage. First stage carries the `[N]<flag>`
/// prefix, state, command, and trailing `&`. Subsequent stages are
/// indented 5 spaces and carry only the PID.
pub fn notification_line_long(job: &Job, flag: char) -> Vec<String> {
    let state = render_state(&job.state);
    let suffix = match job.state {
        JobState::Stopped(_) => "",
        _ => " &",
    };
    let mut lines = Vec::with_capacity(job.pids.len().max(1));
    let first_pid = job.pids.first().copied().unwrap_or(job.pgid);
    lines.push(format!(
        "[{}]{} {} {:<24} {}{}",
        job.id, flag, first_pid, state, job.command, suffix
    ));
    for pid in job.pids.iter().skip(1) {
        lines.push(format!("     {}", pid));
    }
    lines
}
```

For a job with empty `pids` vec (synthetic Done), falls back to
`job.pgid` (which may be 0 for synthetic). Documented as expected
behavior; not a regression.

### New `JobTable::mark_notified`

```rust
pub fn mark_notified(&mut self, ids: &[u32]) {
    for job in self.jobs.iter_mut() {
        if ids.contains(&job.id) {
            job.notified = true;
        }
    }
}
```

### New `JobsArgs` struct + `parse_jobs_args` parser

```rust
struct JobsArgs {
    long: bool,
    pids_only: bool,
    only_new: bool,
    only_running: bool,
    only_stopped: bool,
    targets: Vec<u32>, // empty = no positional filter
}

fn parse_jobs_args(args: &[String], shell: &Shell) -> Result<JobsArgs, ExecOutcome> {
    let mut long = false;
    let mut pids_only = false;
    let mut only_new = false;
    let mut only_running = false;
    let mut only_stopped = false;
    let mut idx = 0;

    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        if let Some(rest) = a.strip_prefix('-') {
            if rest.is_empty() {
                break;
            }
            for c in rest.chars() {
                match c {
                    'l' => long = true,
                    'p' => pids_only = true,
                    'n' => only_new = true,
                    'r' => only_running = true,
                    's' => only_stopped = true,
                    _ => {
                        eprintln!("huck: jobs: -{c}: invalid option");
                        eprintln!("huck: jobs: usage: jobs [-lpnrs] [%spec ...]");
                        return Err(ExecOutcome::Continue(2));
                    }
                }
            }
            idx += 1;
        } else {
            break;
        }
    }

    let mut targets = Vec::new();
    for arg in &args[idx..] {
        if !arg.starts_with('%') {
            eprintln!("huck: jobs: {arg}: no such job");
            return Err(ExecOutcome::Continue(1));
        }
        let id = resolve_spec_or_error(arg, "jobs", shell)?;
        targets.push(id);
    }

    Ok(JobsArgs {
        long, pids_only, only_new, only_running, only_stopped, targets,
    })
}
```

### New `matches_filter` helper

```rust
fn matches_filter(parsed: &JobsArgs, job: &crate::jobs::Job) -> bool {
    if !parsed.targets.is_empty() && !parsed.targets.contains(&job.id) {
        return false;
    }
    if parsed.only_running && !matches!(job.state, crate::jobs::JobState::Running) {
        return false;
    }
    if parsed.only_stopped && !matches!(job.state, crate::jobs::JobState::Stopped(_)) {
        return false;
    }
    if parsed.only_new && job.notified {
        return false;
    }
    true
}
```

### Rewritten `builtin_jobs`

Replace the existing `builtin_jobs` body
(`src/builtins.rs:320-341`):

```rust
fn builtin_jobs(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_jobs_args(args, shell) {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };
    let (current, previous) = shell.jobs.current_and_previous();
    let mut printed_ids: Vec<u32> = Vec::new();
    for job in shell.jobs.iter() {
        if !matches_filter(&parsed, job) {
            continue;
        }
        let flag = if Some(job.id) == current {
            '+'
        } else if Some(job.id) == previous {
            '-'
        } else {
            ' '
        };
        let write_result: std::io::Result<()> = if parsed.pids_only {
            writeln!(out, "{}", job.pgid)
        } else if parsed.long {
            let mut r = Ok(());
            for line in crate::jobs::notification_line_long(job, flag) {
                if let Err(e) = writeln!(out, "{}", line) {
                    r = Err(e);
                    break;
                }
            }
            r
        } else {
            writeln!(out, "{}", crate::jobs::notification_line(job, flag))
        };
        if let Err(e) = write_result {
            eprintln!("huck: jobs: {e}");
            return ExecOutcome::Continue(1);
        }
        printed_ids.push(job.id);
    }
    if parsed.only_new {
        shell.jobs.mark_notified(&printed_ids);
    }
    ExecOutcome::Continue(0)
}
```

### Flag combination rules

- `-l` + `-p` → `-p` wins (pgid-only output). Bash-compat.
- `-r` + `-s` → no matches (no job is both Running and Stopped).
- `-n` + anything else → AND filter (e.g. `-rn` shows
  Running-and-unnotified).
- Positional `%specs` + flags → AND filter (only targets, then
  apply flag filters).

### Error message table

| Condition | Message | Status |
|---|---|---|
| `jobs -x` (unknown flag) | `huck: jobs: -x: invalid option\nhuck: jobs: usage: jobs [-lpnrs] [%spec ...]` | 2 |
| `jobs foo` (positional without `%`) | `huck: jobs: foo: no such job` | 1 |
| `jobs %99` (bad spec) | (existing) `huck: jobs: %99: no such job` | 1 |
| `jobs -r` (no Running) | (no-op) | 0 |
| `jobs -p` (empty table) | (no-op) | 0 |

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod tests`

9 new tests. The existing `mod tests` block in `src/builtins.rs`
(around line 1200+) has access to `run_builtin`, `Shell`, `ExecOutcome`.
Look for the `jobs`-related tests near where existing
`builtin_jobs` tests live (search for `builtin_jobs` in the test
file). Append the 9 new tests to that same mod block:

1. `jobs_l_includes_pid_for_single_stage`
   - Pre-load `shell.jobs.add(1234, vec![1234], "sleep".to_string())`.
   - `run_builtin("jobs", &["-l"], ...)`.
   - Expect status 0, buffer contains `"1234"` AND `"[1]"`.

2. `jobs_l_multistage_shows_all_pids`
   - Pre-load `shell.jobs.add(1234, vec![1234, 1235, 1236], "a | b | c".to_string())`.
   - `run_builtin("jobs", &["-l"], ...)`.
   - Expect status 0, buffer contains all three PIDs, line count ≥ 3.

3. `jobs_p_prints_pgids_only`
   - Pre-load two jobs.
   - `run_builtin("jobs", &["-p"], ...)`.
   - Expect status 0, buffer is exactly two lines, each parses to an int.

4. `jobs_r_filters_running`
   - Pre-load 1 Running + 1 Done.
   - `run_builtin("jobs", &["-r"], ...)`.
   - Expect output contains the Running command but NOT the Done command.

5. `jobs_s_filters_stopped`
   - Pre-load 1 Running + 1 Stopped (via direct field mutation).
   - `run_builtin("jobs", &["-s"], ...)`.
   - Expect output contains Stopped but not Running.

6. `jobs_n_filters_notified_false_and_marks`
   - Pre-load 2 jobs, both with `notified == false` (default).
   - First `run_builtin("jobs", &["-n"], ...)` should print both.
   - Second `run_builtin("jobs", &["-n"], ...)` should print nothing (both now marked).

7. `jobs_positional_spec_filters_to_target`
   - Pre-load 3 jobs.
   - `run_builtin("jobs", &["%2"], ...)`.
   - Expect output contains only job 2's command.

8. `jobs_invalid_flag_returns_usage_status_2`
   - `run_builtin("jobs", &["-x"], ...)`.
   - Expect status 2.

9. `jobs_p_overrides_l`
   - Pre-load 1 job.
   - `run_builtin("jobs", &["-lp"], ...)`.
   - Expect output is the pgid (just digits), NOT the `[N]` prefix.

### Integration tests at `tests/jobs_flags_integration.rs`

2 binary-driven tests:

1. `jobs_p_outputs_bg_pid`
   - Script: `sleep 30 >/dev/null 2>&1 &\nbg=$!\njobs -p\necho LAST=$bg\nexit\n`.
   - Parse the first PID line from `jobs -p`; assert it equals the
     `LAST=` value.

2. `jobs_l_includes_pid_in_listing`
   - Script: `sleep 30 >/dev/null 2>&1 &\nbg=$!\njobs -l\necho LAST=$bg\nexit\n`.
   - Assert stdout contains both `[1]` AND the bg PID value
     (`$LAST`).

### Smoke

`cargo test --all-targets` must pass after the change. PTY flake
tolerated.

## Implementation tasks

1. **Foundation + builtin rewrite + unit tests**:
   - `src/jobs.rs`: add `notification_line_long` + `mark_notified`.
   - `src/builtins.rs`: add `JobsArgs` + `parse_jobs_args` +
     `matches_filter`; rewrite `builtin_jobs`; append 9 unit tests.
2. **Integration tests**: create `tests/jobs_flags_integration.rs`
   with the 2 scenarios.
3. **Docs**: flip M-45 to `[fixed v45]`; change-log; README v45 row.

Three tasks. TDD per task; commit per task.

## Acceptance criteria

- All 9 new unit tests pass.
- Both integration tests pass.
- `cargo test --all-targets` passes (modulo known PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` shows M-45 as `[fixed v45]`.
- `jobs -p` prints one pgid per line.
- `jobs -l` shows PIDs (multi-line for pipelines).
- `jobs -r`, `jobs -s`, `jobs -n`, `jobs %spec` all filter correctly.
- The pre-v45 no-arg `jobs` behavior is unchanged.
