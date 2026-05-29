# huck v45 — `jobs -l/-p/-n/-r/-s` + positional `%spec` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close bash divergence M-45 by adding the five bash `jobs`
flags (`-l`, `-p`, `-n`, `-r`, `-s`) plus combined forms and
positional `%spec` filtering to huck's `jobs` builtin.

**Architecture:** Two files change. `src/jobs.rs` gains a new
`notification_line_long(job, flag) -> Vec<String>` formatter for
bash-faithful multi-line `-l` output and a `JobTable::mark_notified`
helper. `src/builtins.rs` adds `JobsArgs` + `parse_jobs_args` +
`matches_filter` and rewrites `builtin_jobs` as a dispatcher over the
three output modes (default, `-l`, `-p`).

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-jobs-flags-design.md`

**Branch:** `v45-jobs-flags` (created in preamble step P.1).

**Commit trailer convention**:

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v45-jobs-flags
```

Expected: `Switched to a new branch 'v45-jobs-flags'`.

The spec + this plan are committed as the first commit on this
branch (handled by the controller before Task 1 begins).

---

## Task 1: Foundation + builtin rewrite + unit tests

**Files:**
- Modify: `src/jobs.rs` — add `notification_line_long` formatter and
  `JobTable::mark_notified` helper.
- Modify: `src/builtins.rs` — add `JobsArgs` struct + parser +
  `matches_filter`; rewrite `builtin_jobs`; remove or convert the
  broken `jobs_with_args_errors` test at line 1688; append 9 new unit
  tests.

### Step 1.1: Add `notification_line_long` in `src/jobs.rs`

Locate `pub fn notification_line` at `src/jobs.rs:288-295`. Immediately
below it (before `fn decode_status` at line 298), insert:

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

- [ ] **Step 1.1: Insert `notification_line_long`**

### Step 1.2: Add `JobTable::mark_notified` in `src/jobs.rs`

Locate `JobTable::mark_for_nohup` (added in v43; search for `pub fn
mark_for_nohup`). Immediately after that method (still inside the
`impl JobTable` block), add:

```rust
    /// Marks every job in `ids` as notified. Used by `jobs -n` to
    /// consume the state-change flag after printing.
    pub fn mark_notified(&mut self, ids: &[u32]) {
        for job in self.jobs.iter_mut() {
            if ids.contains(&job.id) {
                job.notified = true;
            }
        }
    }
```

- [ ] **Step 1.2: Insert `mark_notified`**

### Step 1.3: Build to confirm `src/jobs.rs` compiles

Run: `cargo build`
Expected: clean. New helpers are unused at this point — Rust treats
unused `pub fn` as non-errors.

- [ ] **Step 1.3: Build clean**

### Step 1.4: Add `JobsArgs` struct + `parse_jobs_args` + `matches_filter` in `src/builtins.rs`

In `src/builtins.rs`, find `fn builtin_jobs` at line 320. **Immediately
above** that function, insert:

```rust
/// Parsed form of the `jobs` argv after flag and positional separation.
struct JobsArgs {
    long: bool,
    pids_only: bool,
    only_new: bool,
    only_running: bool,
    only_stopped: bool,
    targets: Vec<u32>,
}

/// Parses `jobs`'s argv into flags + target ids. Returns
/// `Err(ExecOutcome)` on any usage / lookup failure with the error
/// already printed.
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
        long,
        pids_only,
        only_new,
        only_running,
        only_stopped,
        targets,
    })
}

/// Returns true if `job` passes the filters in `parsed`.
fn matches_jobs_filter(parsed: &JobsArgs, job: &crate::jobs::Job) -> bool {
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

- [ ] **Step 1.4: Insert parser + filter helper**

### Step 1.5: Rewrite `builtin_jobs`

Replace the existing `fn builtin_jobs` body (`src/builtins.rs:320-341`)
with:

```rust
fn builtin_jobs(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_jobs_args(args, shell) {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };
    let (current, previous) = shell.jobs.current_and_previous();
    let mut printed_ids: Vec<u32> = Vec::new();
    for job in shell.jobs.iter() {
        if !matches_jobs_filter(&parsed, job) {
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

- [ ] **Step 1.5: Rewrite `builtin_jobs`**

### Step 1.6: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.6: Build clean**

### Step 1.7: Delete the broken `jobs_with_args_errors` test

In `src/builtins.rs`, find the test at line 1687-1693:

```rust
    #[test]
    fn jobs_with_args_errors() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&["-l".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }
```

This test asserts that `jobs -l` returns status 2 (the pre-v45
"arguments not supported" path). Under v45, `-l` is valid → status 0.
Delete the entire test (the `#[test]` line plus the function and
its closing `}`). The new test `jobs_invalid_flag_returns_usage_status_2`
added in step 1.8 below covers the "unknown flag → status 2" path.

- [ ] **Step 1.7: Delete the broken test**

### Step 1.8: Add the 9 new unit tests

In `src/builtins.rs`, find the `#[cfg(test)] mod tests` block at line
1404. Find the test `jobs_lists_stopped_without_ampersand_suffix`
(around line 1696 — this is one of the existing jobs tests). Append
these 9 new tests immediately after it (still inside `mod tests`):

```rust
    #[test]
    fn jobs_l_includes_pid_for_single_stage() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "sleep 30".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-l".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("1234"), "expected pid 1234 in: {out:?}");
        assert!(out.contains("[1]"), "expected job number in: {out:?}");
    }

    #[test]
    fn jobs_l_multistage_shows_all_pids() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234, 1235, 1236], "a | b | c".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-l".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("1234"), "missing 1234 in: {out:?}");
        assert!(out.contains("1235"), "missing 1235 in: {out:?}");
        assert!(out.contains("1236"), "missing 1236 in: {out:?}");
        let line_count = out.lines().count();
        assert!(line_count >= 3, "expected >=3 lines, got {line_count}: {out:?}");
    }

    #[test]
    fn jobs_p_prints_pgids_only() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string());
        shell.jobs.add(2345, vec![2345], "b".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-p".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 lines, got {lines:?}");
        for l in &lines {
            assert!(
                l.parse::<i32>().is_ok(),
                "expected each line to be an int, got {l:?}"
            );
        }
    }

    #[test]
    fn jobs_r_filters_running() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "running_cmd".to_string()); // %1 Running
        shell.jobs.add_synthetic_done("done_cmd".to_string(), 0);     // %2 Done
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-r".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("running_cmd"), "missing running_cmd: {out:?}");
        assert!(!out.contains("done_cmd"), "should not contain done_cmd: {out:?}");
    }

    #[test]
    fn jobs_s_filters_stopped() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "running_cmd".to_string()); // %1 Running
        shell.jobs.add(2345, vec![2345], "stopped_cmd".to_string()); // %2 then forced Stopped
        shell.jobs.jobs_mut()[1].state = crate::jobs::JobState::Stopped(libc::SIGTSTP);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-s".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("stopped_cmd"), "missing stopped_cmd: {out:?}");
        assert!(!out.contains("running_cmd"), "should not contain running_cmd: {out:?}");
    }

    #[test]
    fn jobs_n_filters_notified_false_and_marks() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string()); // notified=false default
        shell.jobs.add(2345, vec![2345], "b".to_string()); // notified=false default
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-n".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("[1]"), "first call should show [1]: {out:?}");
        assert!(out.contains("[2]"), "first call should show [2]: {out:?}");

        // Second call: both jobs are now marked notified -> empty output.
        let mut buf2: Vec<u8> = Vec::new();
        let outcome2 = run_builtin("jobs", &["-n".to_string()], &mut buf2, &mut shell);
        assert!(matches!(outcome2, ExecOutcome::Continue(0)));
        let out2 = String::from_utf8(buf2).unwrap();
        assert!(out2.is_empty(), "second call should be empty: {out2:?}");
    }

    #[test]
    fn jobs_positional_spec_filters_to_target() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "first_cmd".to_string());  // %1
        shell.jobs.add(2345, vec![2345], "second_cmd".to_string()); // %2
        shell.jobs.add(3456, vec![3456], "third_cmd".to_string());  // %3
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["%2".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("second_cmd"), "missing second_cmd: {out:?}");
        assert!(!out.contains("first_cmd"), "should not contain first_cmd: {out:?}");
        assert!(!out.contains("third_cmd"), "should not contain third_cmd: {out:?}");
    }

    #[test]
    fn jobs_invalid_flag_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-x".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn jobs_p_overrides_l() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("jobs", &["-lp".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        // -p output is just digits + newline, no [N] prefix.
        assert!(!out.contains("[1]"), "expected -p override, got: {out:?}");
        assert_eq!(out.trim(), "1234");
    }
```

- [ ] **Step 1.8: Append the 9 tests**

### Step 1.9: Run the new tests

Run: `cargo test jobs_l_ jobs_p_ jobs_r_ jobs_s_ jobs_n_ jobs_positional_ jobs_invalid_ -- --nocapture`
Expected: all 9 new tests pass.

- [ ] **Step 1.9: New tests pass**

### Step 1.10: Run ALL jobs tests for regression

Run: `cargo test jobs_ -- --nocapture`
Expected: all `jobs_*` tests pass. The previously-failing test
(`jobs_with_args_errors`) is gone; the other pre-existing tests
(`jobs_with_empty_table_prints_nothing_and_returns_zero`,
`jobs_lists_synthetic_done_entry`,
`jobs_lists_stopped_without_ampersand_suffix`) still cover the
no-arg path.

- [ ] **Step 1.10: All jobs tests green**

### Step 1.11: Full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass.

- [ ] **Step 1.11: Full unit suite passes**

### Step 1.12: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.12: Clippy clean**

### Step 1.13: Commit

```bash
git add src/jobs.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: jobs -l/-p/-n/-r/-s + %spec filter (v45 task 1)

Foundation in src/jobs.rs:
- New notification_line_long(job, flag) -> Vec<String> formatter
  for bash-faithful `jobs -l` multi-line output. First stage carries
  [N]<flag> <pid> <state> <command> &. Subsequent stages indented
  5 spaces, PID only.
- New JobTable::mark_notified(&[u32]) helper used by `jobs -n` to
  consume the state-change flag after printing.

Rewrite of builtin_jobs in src/builtins.rs:
- New JobsArgs struct, parse_jobs_args parser (combined flags
  -lpnrs accepted; positional %specs resolved via
  resolve_spec_or_error), matches_jobs_filter helper.
- Three output modes: default single-line, -l multi-line, -p
  pgid-only. -p overrides -l (bash-compat). -r/-s mutually
  exclusive via AND filter (no job is both Running and Stopped).
  -n filters to unnotified; printed jobs are marked notified after
  the loop.
- Positional %specs combine AND with flag filters.

Errors:
- Unknown flag → "huck: jobs: -x: invalid option" + usage + status 2
- Non-% positional → "huck: jobs: <arg>: no such job" + status 1
- %99 (no such job) → status 1 via resolve_spec_or_error

Removed pre-v45 `jobs_with_args_errors` unit test (asserted that
`jobs -l` returns status 2; no longer true). New
`jobs_invalid_flag_returns_usage_status_2` test covers the
unknown-flag path.

9 new unit tests: -l single/multi-stage, -p pgid-only output, -r
filter, -s filter, -n filter + mark-on-print, positional %spec,
unknown flag, -p overrides -l.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.13: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/jobs_flags_integration.rs`

Two binary-driven tests verifying that `jobs -p` and `jobs -l` work
end-to-end through the running `huck` binary.

### Step 2.1: Create the integration test file

Create `tests/jobs_flags_integration.rs` with this exact content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn jobs_p_outputs_bg_pid() {
    // Spawn a bg sleep, capture $!, then run `jobs -p` and verify the
    // pgid printed by jobs -p matches the bg PID. The sleep redirects
    // stdio so wait_with_output() returns promptly when huck exits.
    let script = "sleep 30 >/dev/null 2>&1 &\nbg=$!\njobs -p\necho LAST=$bg\nexit\n";
    let (out, _) = run_capture(script);
    let mut jobs_p_pid: Option<i32> = None;
    let mut last_pid: Option<i32> = None;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("LAST=") {
            if let Ok(n) = rest.parse::<i32>() {
                last_pid = Some(n);
            }
        } else if let Ok(n) = line.trim().parse::<i32>() {
            if jobs_p_pid.is_none() && n > 0 {
                jobs_p_pid = Some(n);
            }
        }
    }
    let jp = jobs_p_pid.unwrap_or_else(|| panic!("no jobs -p pid in: {:?}", out));
    let lp = last_pid.unwrap_or_else(|| panic!("no LAST= line in: {:?}", out));
    assert_eq!(jp, lp, "jobs -p pid ({jp}) != $! ({lp})");
    // Cleanup: bg sleep dies on shell exit via SIGHUP from v43.
}

#[test]
fn jobs_l_includes_pid_in_listing() {
    // `jobs -l` output should contain the bg PID plus the [N] job tag.
    let script = "sleep 30 >/dev/null 2>&1 &\nbg=$!\njobs -l\necho LAST=$bg\nexit\n";
    let (out, _) = run_capture(script);
    let mut last_pid: Option<i32> = None;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("LAST=")
            && let Ok(n) = rest.parse::<i32>()
        {
            last_pid = Some(n);
        }
    }
    let lp = last_pid.unwrap_or_else(|| panic!("no LAST= line in: {:?}", out));
    assert!(
        out.contains(&format!("{lp}")),
        "jobs -l output missing pid {lp}: {:?}",
        out
    );
    assert!(out.contains("[1]"), "jobs -l missing [1] tag: {:?}", out);
}
```

`libc` is already in `[dev-dependencies]` from v41. No `Cargo.toml`
change needed.

- [ ] **Step 2.1: Create the integration test file**

### Step 2.2: Run the new integration suite

Run: `cargo test --test jobs_flags_integration -- --nocapture`
Expected: both tests pass.

If either test fails because `jobs -p` or `jobs -l` is producing
unexpected output: re-run with `--nocapture` to inspect huck's
actual stdout. The likely failure mode is whitespace differences
(e.g. leading spaces on `jobs -p` output, or `jobs -l` format not
matching expectations). Adjust the assertions to be format-tolerant
(use `contains` / `parse` rather than exact-equality) — do NOT
relax the spec-relevant assertions (pid must match, listing must
contain pid and [N]).

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Run the full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. PTY flake
`pty_compound_stage_pipeline_stops_and_resumes` may flake under
load; re-run in isolation if hit.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/jobs_flags_integration.rs
git commit -m "$(cat <<'EOF'
test: jobs -p / jobs -l integration coverage (v45 task 2)

Two binary-driven tests verifying that the new -p and -l output
paths produce the right bg PIDs through the running huck binary.
jobs_p_outputs_bg_pid spawns `sleep 30 >/dev/null 2>&1 &`,
captures $!, runs `jobs -p`, and asserts the printed pgid equals
$!. jobs_l_includes_pid_in_listing verifies `jobs -l` stdout
contains both the bg PID and the [1] job tag.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — flip M-45, change-log entry.
- Modify: `README.md` — v45 row.

### Step 3.1: Flip M-45 in `docs/bash-divergences.md`

Find the M-45 entry in the Job-control section. Currently:

```markdown
- **M-45: `jobs -l`/`-p`/`-n`/`-r`/`-s`** — `[deferred]` medium. huck: rejects all args. bash: per-flag filtering / formatting.
```

Replace with:

```markdown
- **M-45: `jobs -l`/`-p`/`-n`/`-r`/`-s` + positional `%spec`** — `[fixed v45]` medium. All five bash flags supported with combined forms (`-lr`, `-ln`, etc.). `-l` adds PIDs to the listing (bash-faithful multi-line format for pipelines: first stage carries `[N]<flag> <pid> <state> <command> &`, subsequent stages indented 5 spaces with PID only). `-p` prints only pgids, one per line, no decoration (overrides `-l` when both present, bash-compat). `-n` filters to jobs whose state changed since last query (consumes `Job.notified` flag; subsequent `jobs -n` after the same change shows nothing). `-r` filters to Running; `-s` filters to Stopped. Positional `%spec` args filter to specific jobs; combines AND with flag filters.
```

- [ ] **Step 3.1: Flip M-45**

### Step 3.2: Add v45 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most
recent `**2026-05-2X**` entry (v44, M-44, dated 2026-05-29). Add
IMMEDIATELY after it:

```markdown
- **2026-05-29**: M-45 (`jobs` flag filters + positional `%spec`) shipped as v45. New `notification_line_long` formatter in `src/jobs.rs` for bash-faithful multi-line `-l` output; new `JobTable::mark_notified` helper consumed by `-n`. `builtin_jobs` rewritten as a flag parser + filter dispatcher with three output modes (default, `-l`, `-p`). All five bash filters supported with combined forms; `-r`/`-s` are AND-combined (mutually exclusive in practice); `-p` overrides `-l` per bash. Positional `%spec` args resolve via the existing `resolve_spec_or_error`. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v45 row to README

In `README.md`, find the version table. After the v44 row (search
for `| v44       |`), add IMMEDIATELY after it:

```markdown
| v45       | `jobs -l`/`-p`/`-n`/`-r`/`-s` (M-45)                           |
```

Final block:

```markdown
| v43       | `disown -a`/`-r`/`-h` + SIGHUP-on-exit (M-43)                  |
| v44       | `disown` accepts bare PID (M-44)                               |
| v45       | `jobs -l`/`-p`/`-n`/`-r`/`-s` (M-45)                           |
```

Match column padding to v43/v44 (count actual trailing spaces in
the file).

- [ ] **Step 3.3: Add README v45 row**

### Step 3.4: Full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo known PTY flake).

- [ ] **Step 3.4: Full suite green**

### Step 3.5: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.5: Clippy clean**

### Step 3.6: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-45 fixed; v45 in README

Job-control section: M-45 (`jobs -l/-p/-n/-r/-s` + positional
`%spec`) flipped from [deferred] to [fixed v45] with descriptive
text covering all five flags, the bash-faithful multi-line -l
format for pipelines, the -p-overrides-l rule, the -n
notification-consumption semantics, and AND filter composition.

Change log: 2026-05-29 v45 entry summarizing the new formatter,
mark_notified helper, and dispatcher rewrite.

README: v45 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the full
   diff (`main..v45-jobs-flags`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v45.
