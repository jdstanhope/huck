# huck v40 — `wait -n` + multi-arg `wait` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close bash divergences M-37 (`wait -n` with optional `-p VAR`)
and M-38 (multi-arg `wait` returning status of last) in huck's `wait`
builtin.

**Architecture:** All changes confined to `src/builtins.rs` (rewrite the
`builtin_wait` dispatcher + add a flag/positional parser, two small
internal types, and three new helpers that reuse the existing
`waitpid`-poll machinery from `wait_all` / `wait_for_job` /
`wait_for_pid`). One new integration test file. No new modules. No
parser/lexer/AST/expansion changes.

**Tech Stack:** Rust. `libc::waitpid` + `WNOHANG | WUNTRACED` for the
poll primitive (already used by existing helpers).

**Spec:** `docs/superpowers/specs/2026-05-28-huck-wait-bundle-design.md`

**Branch:** `v40-wait-bundle` (to be created in preamble step P.1).

**Commit trailer convention** (every commit in this iteration):

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v40-wait-bundle
```

Expected: `Switched to a new branch 'v40-wait-bundle'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: Builtin core — dispatcher + parser + helpers + unit tests

**Files:**
- Modify: `src/builtins.rs` — replace `builtin_wait` (`src/builtins.rs:342-364`),
  add new types and helpers, add 10 new unit tests, rename one existing
  test.

### Step 1.1: Add the new internal types and parser

Right above the existing `fn builtin_wait` (currently at
`src/builtins.rs:342`), insert:

```rust
/// A single positional `wait` target. Built by `parse_wait_args` from a
/// `%spec` (resolved to a job id) or a positive integer PID.
enum WaitTarget {
    Job(u32),
    Pid(i32),
}

/// Parsed form of the `wait` argv after flag and positional separation.
struct WaitArgs {
    wait_any: bool,
    pid_var: Option<String>,
    targets: Vec<WaitTarget>,
}

/// Parses `wait`'s argv into flags + targets. Returns `Err(ExecOutcome)`
/// on any usage / parse failure, with the appropriate stderr message
/// already printed.
fn parse_wait_args(args: &[String], shell: &Shell) -> Result<WaitArgs, ExecOutcome> {
    let mut wait_any = false;
    let mut pid_var: Option<String> = None;
    let mut idx = 0;

    while idx < args.len() {
        let a = &args[idx];
        match a.as_str() {
            "-n" => {
                wait_any = true;
                idx += 1;
            }
            "-p" => {
                if idx + 1 >= args.len() {
                    eprintln!("huck: wait: -p: option requires a variable name");
                    return Err(ExecOutcome::Continue(2));
                }
                pid_var = Some(args[idx + 1].clone());
                idx += 2;
            }
            "--" => {
                idx += 1;
                break;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: wait: {s}: invalid option");
                eprintln!("huck: wait: usage: wait [-n] [-p var] [id ...]");
                return Err(ExecOutcome::Continue(2));
            }
            _ => break,
        }
    }

    if pid_var.is_some() && !wait_any {
        eprintln!("huck: wait: -p: option requires -n");
        return Err(ExecOutcome::Continue(2));
    }

    let mut targets = Vec::with_capacity(args.len() - idx);
    while idx < args.len() {
        let arg = &args[idx];
        if arg.starts_with('%') {
            let id = resolve_spec_or_error(arg, "wait", shell)?;
            targets.push(WaitTarget::Job(id));
        } else {
            match arg.parse::<i32>() {
                Ok(pid) if pid > 0 => targets.push(WaitTarget::Pid(pid)),
                _ => {
                    eprintln!("huck: wait: {arg}: not a pid or valid job spec");
                    return Err(ExecOutcome::Continue(2));
                }
            }
        }
        idx += 1;
    }

    Ok(WaitArgs { wait_any, pid_var, targets })
}
```

- [ ] **Step 1.1: Insert the types and parser**

Add the block above immediately before `fn builtin_wait`.

### Step 1.2: Replace the existing `builtin_wait`

Replace the current `fn builtin_wait` (`src/builtins.rs:342-364`) with:

```rust
fn builtin_wait(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    let parsed = match parse_wait_args(args, shell) {
        Ok(p) => p,
        Err(outcome) => return outcome,
    };

    match (parsed.wait_any, parsed.targets.len()) {
        (false, 0) => wait_all(shell),
        (false, 1) => match &parsed.targets[0] {
            WaitTarget::Job(id) => wait_for_job(*id, shell),
            WaitTarget::Pid(pid) => wait_for_pid(*pid, shell),
        },
        (false, _) => wait_for_all(parsed.targets, shell),
        (true, 0) => wait_any_pending(parsed.pid_var, shell),
        (true, _) => wait_any_of(parsed.targets, parsed.pid_var, shell),
    }
}
```

- [ ] **Step 1.2: Apply the replacement**

### Step 1.3: Add `wait_for_all` helper (M-38)

Add after the existing `wait_for_pid` (which ends around
`src/builtins.rs:442`) and before `fn check_sigint`:

```rust
/// Multi-arg `wait` (M-38): wait sequentially for each target. Return
/// the status of the LAST target waited.
fn wait_for_all(targets: Vec<WaitTarget>, shell: &mut Shell) -> ExecOutcome {
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
}
```

- [ ] **Step 1.3: Insert `wait_for_all`**

### Step 1.4: Add `wait_any_pending` helper (M-37 bare `-n`)

Add immediately after `wait_for_all`:

```rust
/// `wait -n` with no positional args (M-37 bare). Snapshot the set of
/// currently-Running job ids at entry, then poll until one of them
/// transitions to `Done(c)` or `Signaled(s)`. Returns 127 immediately if
/// no Running jobs at entry, or if all snapshotted jobs vanish from the
/// table mid-wait. Captures the finished job's pgid into `$pid_var`
/// when provided; on the 127 path sets `$pid_var = ""`.
fn wait_any_pending(pid_var: Option<String>, shell: &mut Shell) -> ExecOutcome {
    let snapshot: Vec<u32> = shell
        .jobs
        .iter()
        .filter(|j| matches!(j.state, crate::jobs::JobState::Running))
        .map(|j| j.id)
        .collect();

    if snapshot.is_empty() {
        if let Some(name) = &pid_var {
            shell.set(name, String::new());
        }
        return ExecOutcome::Continue(127);
    }

    loop {
        // Look for a snapshotted job that's now terminal.
        let found = shell.jobs.iter().find_map(|j| {
            if !snapshot.contains(&j.id) {
                return None;
            }
            match j.state {
                crate::jobs::JobState::Done(c) => Some((j.pgid, c)),
                crate::jobs::JobState::Signaled(s) => Some((j.pgid, 128 + s)),
                _ => None,
            }
        });
        if let Some((pgid, status)) = found {
            if let Some(name) = &pid_var {
                shell.set(name, pgid.to_string());
            }
            return ExecOutcome::Continue(status);
        }

        // If every snapshotted job is gone from the table without being
        // observed as terminal (e.g. external `disown`), bail with 127.
        let still_present = shell
            .jobs
            .iter()
            .any(|j| snapshot.contains(&j.id));
        if !still_present {
            if let Some(name) = &pid_var {
                shell.set(name, String::new());
            }
            return ExecOutcome::Continue(127);
        }

        if check_sigint(shell) {
            return ExecOutcome::Continue(130);
        }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}
```

- [ ] **Step 1.4: Insert `wait_any_pending`**

### Step 1.5: Add `wait_any_of` helper (M-37 with target list)

Add immediately after `wait_any_pending`:

```rust
/// `wait -n` with explicit target list (M-37 with subset). Returns the
/// status of the first listed target to finish. Captures the finished
/// PID into `$pid_var` when provided — for `WaitTarget::Job(id)` that's
/// the job's pgid; for `WaitTarget::Pid(pid)` that's the literal PID.
/// If at entry no target can ever finish (all unknown / not children),
/// returns 127 with `$pid_var = ""`.
fn wait_any_of(
    targets: Vec<WaitTarget>,
    pid_var: Option<String>,
    shell: &mut Shell,
) -> ExecOutcome {
    // Pre-check: is anyone already terminal?
    if let Some((pid, status)) = check_targets_terminal(&targets, shell) {
        if let Some(name) = &pid_var {
            shell.set(name, pid.to_string());
        }
        return ExecOutcome::Continue(status);
    }

    // Pre-check: can anyone still finish from our side?
    let any_active = targets.iter().any(|t| match t {
        WaitTarget::Job(id) => shell.jobs.iter().any(|j| j.id == *id),
        WaitTarget::Pid(pid) => {
            // A bare PID we don't recognize might still be a child;
            // probe with waitpid(pid, WNOHANG). ECHILD => not (or no
            // longer) ours.
            let mut s: libc::c_int = 0;
            let r = unsafe { libc::waitpid(*pid, &mut s, libc::WNOHANG | libc::WUNTRACED) };
            if r > 0 {
                shell.jobs.reap(r, s);
                // It just finished — keep it as a candidate for the
                // main loop's pre-check next iteration. For now treat
                // as active so we don't bail with 127.
                true
            } else {
                r == 0
            }
        }
    });
    if !any_active {
        if let Some(name) = &pid_var {
            shell.set(name, String::new());
        }
        return ExecOutcome::Continue(127);
    }

    // Re-check after any probes triggered a reap above.
    if let Some((pid, status)) = check_targets_terminal(&targets, shell) {
        if let Some(name) = &pid_var {
            shell.set(name, pid.to_string());
        }
        return ExecOutcome::Continue(status);
    }

    loop {
        if check_sigint(shell) {
            return ExecOutcome::Continue(130);
        }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        if let Some((pid, st)) = check_targets_terminal(&targets, shell) {
            if let Some(name) = &pid_var {
                shell.set(name, pid.to_string());
            }
            return ExecOutcome::Continue(st);
        }
    }
}

/// Returns `(captured_pid, exit_status)` for the first target that is
/// currently terminal, or `None`.
///
/// For `WaitTarget::Job(id)` the captured pid is the job's `pgid`. For
/// `WaitTarget::Pid(pid)` the captured pid is the literal PID arg.
fn check_targets_terminal(targets: &[WaitTarget], shell: &Shell) -> Option<(i32, i32)> {
    for t in targets {
        match t {
            WaitTarget::Job(id) => {
                if let Some(job) = shell.jobs.iter().find(|j| j.id == *id) {
                    match job.state {
                        crate::jobs::JobState::Done(c) => return Some((job.pgid, c)),
                        crate::jobs::JobState::Signaled(s) => {
                            return Some((job.pgid, 128 + s))
                        }
                        _ => {}
                    }
                }
            }
            WaitTarget::Pid(pid) => {
                // Find the job that owns this pid; if its state is
                // terminal AND this pid was the last stage (so the
                // job's overall status reflects this pid's exit), use
                // the job's status. Otherwise the per-pid status isn't
                // separately tracked, so fall back to the job's
                // overall status when the job is terminal.
                if let Some(job) = shell.jobs.iter().find(|j| j.pids.contains(pid)) {
                    match job.state {
                        crate::jobs::JobState::Done(c) => return Some((*pid, c)),
                        crate::jobs::JobState::Signaled(s) => {
                            return Some((*pid, 128 + s))
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    None
}
```

- [ ] **Step 1.5: Insert `wait_any_of` and `check_targets_terminal`**

### Step 1.6: Build to confirm compilation

Run: `cargo build`
Expected: clean build (some `#[allow(dead_code)]` may already cover any
fields you don't read; verify clippy at the end of the task).

### Step 1.7: Rename + replace the broken existing test

In `src/builtins.rs`, find `fn wait_with_multiple_args_returns_usage_status_2`
(around `src/builtins.rs:1694`). Replace the whole test with:

```rust
    #[test]
    fn wait_multiarg_unparseable_returns_usage_status_2() {
        // Multi-arg wait is now valid; only bad arg syntax should usage-error.
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["1234".to_string(), "abc".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }
```

- [ ] **Step 1.7: Rename and replace the broken test**

### Step 1.8: Add the 9 new unit tests

Append these inside the same `#[cfg(test)] mod tests` block in
`src/builtins.rs`, after the existing wait tests block (right before
the `mod kill_tests` heading at `src/builtins.rs:1745`):

```rust
    #[test]
    fn wait_multiarg_two_done_returns_last_status() {
        // Two synthetic Done jobs: %1=Done(0), %2=Done(5). wait %1 %2 → 5.
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        shell.jobs.add_synthetic_done("exit 5".to_string(), 5);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(5)));
    }

    #[test]
    fn wait_multiarg_unparseable_rejects_before_waiting() {
        // First arg is good but second is bad — must return 2 without
        // calling waitpid. We verify by checking that the first arg
        // (which is a synthetic Done job) was NOT consumed: the job is
        // still present in the table afterwards.
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["%1".to_string(), "abc".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_n_with_no_jobs_returns_127() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-n".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(127)));
    }

    #[test]
    fn wait_n_with_only_done_jobs_returns_127() {
        // Snapshot includes only Running jobs; if all jobs are already
        // Done, snapshot is empty → 127.
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-n".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(127)));
    }

    #[test]
    fn wait_n_with_explicit_already_done_returns_its_status() {
        // wait -n %1 where %1 is already Done(7) → 7 immediately.
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("exit 7".to_string(), 7);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["-n".to_string(), "%1".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(7)));
    }

    #[test]
    fn wait_n_p_var_captures_pgid_via_explicit_target() {
        // wait -n -p PID %1 where %1 has synthetic Done(0) and pgid 0
        // (add_synthetic_done sets pgid=0). After the call $PID == "0".
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("true".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &[
                "-n".to_string(),
                "-p".to_string(),
                "PID".to_string(),
                "%1".to_string(),
            ],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("PID").as_deref(), Some("0"));
    }

    #[test]
    fn wait_p_without_n_is_usage_error() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["-p".to_string(), "PID".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_n_p_without_var_name_is_usage_error() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "wait",
            &["-n".to_string(), "-p".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_invalid_flag_is_usage_error() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("wait", &["-x".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }
```

That's 9 new tests + the renamed one = 10 tests for the v40 surface.

- [ ] **Step 1.8: Add the 9 new tests**

### Step 1.9: Run the new and existing wait tests

Run: `cargo test wait_ -- --nocapture`
Expected: all `wait_*` unit tests pass (the 6 existing + 10 new).

- [ ] **Step 1.9: Confirm tests pass**

### Step 1.10: Run the full lib test suite to catch regressions

Run: `cargo test --bin huck`
Expected: all unit tests pass.

If `cargo test --bin huck` complains about missing target, fall back to
`cargo test --lib` or, for this binary-crate project, just
`cargo test` (skipping integration tests) — match whatever the project
README suggests.

- [ ] **Step 1.10: Full unit-test pass**

### Step 1.11: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

If clippy flags `WaitArgs` fields as unused-after-destructure, prefix
the unused position with `_` or add `#[allow(dead_code)]` to the
struct — both are acceptable.

- [ ] **Step 1.11: Clippy clean**

### Step 1.12: Commit

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: wait -n + multi-arg wait (v40 task 1)

Rewrite builtin_wait as a small dispatcher over a new parse_wait_args
helper. Flag pass handles -n and -p VAR (with the bash 5.1 rule that -p
requires -n); positional pass parses each remaining arg as either a
%spec (via the existing resolve_spec_or_error) or a positive PID.
Targets land in a WaitTarget enum (Job(u32) | Pid(i32)).

Dispatch by (wait_any, targets.len()):
  (false, 0) → existing wait_all
  (false, 1) → existing wait_for_job / wait_for_pid
  (false, N) → new wait_for_all (M-38: status of last)
  (true,  0) → new wait_any_pending (M-37 bare: snapshot Running ids,
                poll until one transitions, capture pgid into $VAR)
  (true,  N) → new wait_any_of (M-37 subset: filter to listed targets)

`-p VAR` captures the finished job's pgid for %spec targets, the
literal PID for PID targets. Empty-jobs and all-targets-unknown paths
return 127 immediately and set $VAR = "" when -p is present.

The pre-existing wait_with_multiple_args_returns_usage_status_2 test
(asserted multi-arg = usage error) is replaced with
wait_multiarg_unparseable_returns_usage_status_2 to reflect the new
semantics. Nine additional tests cover multi-arg last-status,
multi-arg early rejection, -n empty / -n already-done / -n -p capture,
and the four flag-validation paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.12: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/wait_integration.rs`

Five binary-driven tests using the established integration harness
pattern (mirrors `tests/arith_completion_integration.rs` and
`tests/ansi_c_quoting_integration.rs`).

### Step 2.1: Create the integration test file

Create `tests/wait_integration.rs` with this content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
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
fn wait_multiarg_all_succeed() {
    // Both bg jobs exit 0; wait %1 %2; echo $? → 0
    let (out, _) = run("(true) &\n(true) &\nwait %1 %2\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {:?}", out);
}

#[test]
fn wait_multiarg_returns_last_status() {
    // First bg job exits 5, second exits 3. wait %1 %2 → 3.
    let (out, _) = run("(exit 5) &\n(exit 3) &\nwait %1 %2\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "3"), "stdout: {:?}", out);
}

#[test]
fn wait_n_returns_first_finished_status() {
    // One bg job sleeps briefly then exits 7. wait -n → 7.
    let (out, _) = run("(sleep 0.05; exit 7) &\nwait -n\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "7"), "stdout: {:?}", out);
}

#[test]
fn wait_n_with_no_jobs_returns_127() {
    let (out, _) = run("wait -n\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "127"), "stdout: {:?}", out);
}

#[test]
fn wait_n_p_captures_pid_into_var() {
    // wait -n -p FINPID against a single bg job; echo $FINPID prints the
    // job's pgid (a positive integer). The test asserts the first
    // captured line parses to a positive i32 and the exit status is the
    // bg job's exit code (3).
    let (out, _) = run(
        "(sleep 0.05; exit 3) &\nwait -n -p FINPID\necho \"pid=$FINPID\"\necho $?\nexit\n",
    );
    let mut pid_line = None;
    let mut status_line = None;
    for l in out.lines() {
        if let Some(rest) = l.strip_prefix("pid=") {
            pid_line = Some(rest.to_string());
        } else if l == "3" {
            status_line = Some(l.to_string());
        }
    }
    let pid = pid_line.expect(&format!("no pid= line in stdout: {:?}", out));
    let parsed: i32 = pid.parse().expect(&format!("pid not an integer: {pid:?}"));
    assert!(parsed > 0, "pid was not positive: {parsed}");
    assert!(status_line.is_some(), "no status line: {:?}", out);
}
```

- [ ] **Step 2.1: Create the integration test file**

### Step 2.2: Run the new integration suite

Run: `cargo test --test wait_integration -- --nocapture`
Expected: all 5 tests pass.

If any test races (e.g. `wait -n` returning before the sleep finishes
because the shell takes >50 ms to launch the bg job): bump the sleep to
0.1 in that test and retry. Do NOT relax the assertion.

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Run the full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. Known PTY flake
`pty_compound_stage_pipeline_stops_and_resumes` may flake under load —
re-run in isolation
(`cargo test --test pty_interactive pty_compound_stage_pipeline_stops_and_resumes`)
to confirm if hit; tolerate per prior iterations.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/wait_integration.rs
git commit -m "$(cat <<'EOF'
test: wait -n + multi-arg integration coverage (v40 task 2)

Five binary-driven tests covering: multi-arg wait with all-success,
multi-arg returning status of last waited target (5 then 3 → 3), wait
-n returning the bg job's exit code, wait -n on an empty job list
returning 127, and wait -n -p VAR populating $VAR with a positive
integer PID alongside the correct exit status.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs + full-suite verify

**Files:**
- Modify: `docs/bash-divergences.md`
- Modify: `README.md`

### Step 3.1: Flip M-37 and M-38 in the Job-control section

In `docs/bash-divergences.md`, find these two consecutive lines (around
lines 176-177):

```markdown
- **M-37: `wait -n`** — `[deferred]` medium. huck: rejects `-n`. bash: waits for any one job to finish.
- **M-38: `wait` with multiple args** — `[deferred]` medium. huck: rejects more than one arg. bash: accepts a list.
```

Replace them with:

```markdown
- **M-37: `wait -n`** — `[fixed v40]` medium. `wait -n` waits for the next job to finish; with no positional args it considers all currently-Running jobs; with a target list it restricts to those. Bash 5.1 `-p VAR` flag also supported — captures the finished job's pgid (for `%spec` targets) or literal PID (for PID targets) into `$VAR`. Empty job list returns 127 immediately and clears `$VAR`. `-p` without `-n` is a usage error. `-f` and `-np` (combined-flag form) deferred.
- **M-38: `wait` with multiple args** — `[fixed v40]` medium. `wait PID1 PID2 …` and `wait %1 %2 …` (or mixed) now supported. Targets are waited for sequentially; exit status is the status of the LAST one waited. Unparseable args trigger usage error 2 BEFORE any waiting begins.
```

- [ ] **Step 3.1: Flip M-37 and M-38**

### Step 3.2: Add the v40 change-log entry

In `docs/bash-divergences.md`, find the `## Change log` section and the
most recent `**2026-05-28**` entry (about v39). Add immediately after it:

```markdown
- **2026-05-28**: M-37 (`wait -n` with optional `-p VAR`) and M-38 (multi-arg `wait`) shipped together as v40. `builtin_wait` rewritten as a flag/positional parser (`parse_wait_args`) feeding a 5-way dispatch over `(wait_any, targets.len())`. Three new helpers (`wait_for_all`, `wait_any_pending`, `wait_any_of`) reuse the existing `waitpid(-1, WNOHANG)`-poll machinery. `-p VAR` captures the finished job's pgid (for `%spec` targets) or literal PID (for PID targets). All changes confined to `src/builtins.rs`. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add the v40 row to the README version table

In `README.md`, find the version-table block (search for the existing
`| v39       | ANSI-C quoting` line). Add immediately after it:

```markdown
| v40       | `wait -n` + multi-arg `wait` (M-37 + M-38)                    |
```

So the final block reads:

```markdown
| v38       | Arithmetic completion (M-55 + M-56 + M-57 + `**`)              |
| v39       | ANSI-C quoting `$'…'` (M-28)                                   |
| v40       | `wait -n` + multi-arg `wait` (M-37 + M-38)                    |
```

Match the column padding of v38/v39 (count spaces before the trailing
`|` so the right pipe lines up visually).

- [ ] **Step 3.3: Add README v40 row**

### Step 3.4: Run the full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo the known PTY flake — re-run in
isolation if hit).

- [ ] **Step 3.4: Full suite green**

### Step 3.5: Run clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.5: Clippy clean**

### Step 3.6: Commit Task 3

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-37 and M-38 fixed; v40 in README

Job-control section: M-37 (`wait -n`) and M-38 (multi-arg `wait`)
flipped from [deferred] to [fixed v40] with descriptive text covering
the `-p VAR` capture rule, the multi-arg "status of last" semantics,
the 127-on-empty rule, and the explicit -f/-np deferrals.

Change log: 2026-05-28 v40 entry summarizing the dispatcher rewrite
and the three new helpers.

README: v40 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land, the controller should:

1. Run `cargo test --all-targets` once more from a clean checkout.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`: the
   docs preamble commit (spec + plan), task 1, task 2, task 3.
4. Dispatch the final code-reviewer subagent over the full diff
   (`main..v40-wait-bundle`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update the
   `huck iterations` memory entry with v40.
