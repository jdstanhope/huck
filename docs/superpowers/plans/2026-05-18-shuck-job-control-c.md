# Sub-project C (job specs, kill, disown) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `%N`/`%+`/`%%`/`%-` job-specifier parser, introduce `kill` and `disown` builtins, and extend `fg`/`bg`/`wait` to accept those specs (plus `wait PID`).

**Architecture:** New module `src/job_spec.rs` owns parsing. `JobTable` gains `resolve(&JobSpec) -> Option<u32>`. Each job-aware builtin uses a shared `resolve_spec_or_error` helper. `kill` and `disown` follow the existing `is_builtin` + `run_builtin` dispatch pattern. No lexer/parser changes — job specs are interpreted by builtins at runtime.

**Tech Stack:** Rust 2024 edition, `libc` for `kill`/`killpg`/`waitpid` and signal constants. No new dependencies.

**Branch:** `feature/job-control-c` off `main`.

---

## Pre-flight

- [ ] **Step 0a: Create the feature branch from main**

```bash
git checkout main
git pull --ff-only
git checkout -b feature/job-control-c
```

- [ ] **Step 0b: Baseline — confirm clean build and all tests pass**

Run: `cargo build && cargo test`
Expected: clean build, `test result: ok. 214 passed; 0 failed`.

---

## Task 1: `JobSpec` parser module

**Files:**
- Create: `src/job_spec.rs`
- Modify: `src/main.rs` (add `mod job_spec;`)
- Test: `src/job_spec.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Create the empty module file with the public surface**

Create `src/job_spec.rs`:

```rust
//! Job-spec parser. Job specs are runtime-only — the lexer/parser
//! doesn't know about them. Builtins call `parse_job_spec` on any
//! argument starting with `%`.

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum JobSpec {
    Id(u32),
    Current,
    Previous,
}

#[derive(Debug, PartialEq, Eq)]
pub enum JobSpecError {
    Empty,
    BadNumber,
    BadSymbol,
}

pub fn parse_job_spec(_s: &str) -> Result<JobSpec, JobSpecError> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
}
```

- [ ] **Step 2: Register the module in `src/main.rs`**

Add `mod job_spec;` to the existing `mod` list in `src/main.rs` (alphabetically next to `mod jobs;`).

Run: `cargo build`
Expected: clean build (with one unused-import-style warning about the module — that's fine for the moment).

- [ ] **Step 3: Write the failing tests**

In `src/job_spec.rs::tests`, add:

```rust
#[test]
fn parse_percent_alone_is_empty_error() {
    assert_eq!(parse_job_spec("%"), Err(JobSpecError::Empty));
}

#[test]
fn parse_percent_plus_is_current() {
    assert_eq!(parse_job_spec("%+"), Ok(JobSpec::Current));
}

#[test]
fn parse_percent_percent_is_current() {
    assert_eq!(parse_job_spec("%%"), Ok(JobSpec::Current));
}

#[test]
fn parse_percent_minus_is_previous() {
    assert_eq!(parse_job_spec("%-"), Ok(JobSpec::Previous));
}

#[test]
fn parse_percent_digits_is_id() {
    assert_eq!(parse_job_spec("%1"), Ok(JobSpec::Id(1)));
    assert_eq!(parse_job_spec("%42"), Ok(JobSpec::Id(42)));
    assert_eq!(parse_job_spec("%999"), Ok(JobSpec::Id(999)));
}

#[test]
fn parse_percent_digits_with_trailing_garbage_is_bad_number() {
    assert_eq!(parse_job_spec("%1x"), Err(JobSpecError::BadNumber));
    assert_eq!(parse_job_spec("%-1"), Ok(JobSpec::Previous));   // %- prefix takes precedence
}

#[test]
fn parse_percent_letters_is_bad_symbol() {
    assert_eq!(parse_job_spec("%abc"), Err(JobSpecError::BadSymbol));
}

#[test]
fn parse_percent_tilde_is_bad_symbol() {
    assert_eq!(parse_job_spec("%~"), Err(JobSpecError::BadSymbol));
}

#[test]
fn parse_input_without_percent_is_bad_symbol() {
    // Defensive: callers should not pass non-% input, but if they do,
    // we error rather than panic.
    assert_eq!(parse_job_spec("1"), Err(JobSpecError::BadSymbol));
    assert_eq!(parse_job_spec(""), Err(JobSpecError::BadSymbol));
}
```

- [ ] **Step 4: Run the tests — confirm failure**

Run: `cargo test job_spec::tests`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 5: Implement `parse_job_spec`**

Replace the `todo!()` body in `src/job_spec.rs`:

```rust
pub fn parse_job_spec(s: &str) -> Result<JobSpec, JobSpecError> {
    let rest = match s.strip_prefix('%') {
        Some(r) => r,
        None => return Err(JobSpecError::BadSymbol),
    };
    if rest.is_empty() {
        return Err(JobSpecError::Empty);
    }
    match rest {
        "+" | "%" => return Ok(JobSpec::Current),
        "-" => return Ok(JobSpec::Previous),
        _ => {}
    }
    if rest.starts_with('-') {
        // "%-1", "%-x" — we already matched plain "%-" above, so anything
        // longer starting with '-' is malformed.
        return Err(JobSpecError::BadSymbol);
    }
    if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return rest
            .parse::<u32>()
            .map(JobSpec::Id)
            .map_err(|_| JobSpecError::BadNumber);
    }
    Err(JobSpecError::BadSymbol)
}
```

- [ ] **Step 6: Adjust the `%-1` test if needed**

The test `parse_percent_digits_with_trailing_garbage_is_bad_number` includes:
```rust
assert_eq!(parse_job_spec("%-1"), Ok(JobSpec::Previous));   // %- prefix takes precedence
```

This needs to match what the parser actually does. With the implementation above, `%-1` falls into the `rest.starts_with('-')` branch (since "-1" doesn't equal "-") and returns `Err(BadSymbol)`. Update the test assertion:

```rust
assert_eq!(parse_job_spec("%-1"), Err(JobSpecError::BadSymbol));
```

(Bash actually treats `%-1` as an error too, so this matches reference behavior.)

- [ ] **Step 7: Run tests — should pass**

Run: `cargo test job_spec::tests`
Expected: PASS (9 tests).

- [ ] **Step 8: Run full suite**

Run: `cargo test`
Expected: 223 passed (214 baseline + 9 new).

- [ ] **Step 9: Commit**

```bash
git add src/main.rs src/job_spec.rs
git commit -m "feat: add JobSpec parser module (%N, %+, %%, %-)"
```

---

## Task 2: `JobTable::resolve` + `resolve_spec_or_error` helper

**Files:**
- Modify: `src/jobs.rs` — add `resolve` method on `JobTable`.
- Modify: `src/builtins.rs` — add `resolve_spec_or_error` helper near `builtin_fg`.
- Test: `src/jobs.rs`

- [ ] **Step 1: Write the failing tests for `JobTable::resolve`**

Add to `src/jobs.rs::tests`:

```rust
#[test]
fn resolve_id_returns_matching_id() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());
    let _ = t.add(200, vec![200], "b".to_string());
    let spec = crate::job_spec::JobSpec::Id(2);
    assert_eq!(t.resolve(&spec), Some(2));
}

#[test]
fn resolve_id_missing_returns_none() {
    let t = JobTable::new();
    let spec = crate::job_spec::JobSpec::Id(99);
    assert_eq!(t.resolve(&spec), None);
}

#[test]
fn resolve_current_uses_current_id() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());
    let _ = t.add(200, vec![200], "b".to_string());
    assert_eq!(t.resolve(&crate::job_spec::JobSpec::Current), Some(2));
}

#[test]
fn resolve_previous_returns_second_most_recent() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());
    let _ = t.add(200, vec![200], "b".to_string());
    assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), Some(1));
}

#[test]
fn resolve_previous_returns_none_when_only_one_job() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());
    assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), None);
}
```

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test jobs::tests::resolve`
Expected: FAIL — `resolve` does not exist on `JobTable`.

- [ ] **Step 3: Implement `JobTable::resolve`**

Add to `src/jobs.rs`, in the existing `impl JobTable` block, just below `has_pending`:

```rust
/// Resolves a JobSpec to a job id, if any matching job exists.
pub fn resolve(&self, spec: &crate::job_spec::JobSpec) -> Option<u32> {
    match spec {
        crate::job_spec::JobSpec::Id(id) => {
            self.jobs.iter().find(|j| j.id == *id).map(|j| j.id)
        }
        crate::job_spec::JobSpec::Current => self.current_id(),
        crate::job_spec::JobSpec::Previous => {
            let (_, prev) = self.current_and_previous();
            prev
        }
    }
}
```

- [ ] **Step 4: Run — should pass**

Run: `cargo test jobs::tests::resolve`
Expected: PASS (5 tests).

- [ ] **Step 5: Add the builtin-side helper**

In `src/builtins.rs`, just above `builtin_fg`, add:

```rust
/// Parses `arg` as a job spec and resolves it to a job id. On parse or
/// resolution failure, prints a `shuck: <builtin>: ...` error to stderr
/// and returns `Err(ExecOutcome::Continue(1))` so the caller can `?` it.
fn resolve_spec_or_error(
    arg: &str,
    builtin: &str,
    shell: &Shell,
) -> Result<u32, ExecOutcome> {
    let spec = crate::job_spec::parse_job_spec(arg).map_err(|_| {
        eprintln!("shuck: {builtin}: {arg}: bad job spec");
        ExecOutcome::Continue(1)
    })?;
    shell.jobs.resolve(&spec).ok_or_else(|| {
        eprintln!("shuck: {builtin}: {arg}: no such job");
        ExecOutcome::Continue(1)
    })
}
```

- [ ] **Step 6: Build to confirm everything compiles (helper is unused for now)**

Run: `cargo build`
Expected: clean (with one `dead_code` warning for `resolve_spec_or_error`, which Tasks 3–7 will consume).

- [ ] **Step 7: Run all tests**

Run: `cargo test`
Expected: 228 passed (223 + 5 new).

- [ ] **Step 8: Commit**

```bash
git add src/jobs.rs src/builtins.rs
git commit -m "feat: JobTable::resolve + builtin resolve_spec_or_error helper"
```

---

## Task 3: extend `builtin_fg` to accept `%spec`

**Files:**
- Modify: `src/builtins.rs::builtin_fg`
- Test: `src/builtins.rs::fg_bg_tests`

- [ ] **Step 1: Write failing tests**

Add to `src/builtins.rs::fg_bg_tests`:

```rust
#[test]
fn fg_with_bad_job_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("fg", &["%abc".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn fg_with_no_such_job_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("fg", &["%99".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn fg_with_non_percent_arg_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("fg", &["1".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn fg_with_multiple_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "fg",
        &["%1".to_string(), "%2".to_string()],
        &mut buf,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}
```

- [ ] **Step 2: Run — confirm the usage/bad-spec tests fail**

Run: `cargo test builtins::fg_bg_tests::fg_with`
Expected: FAIL — current `fg` returns 2 for ANY arg (existing behavior is "arguments not supported"), so `fg_with_bad_job_spec_errors_status_1` and `fg_with_no_such_job_spec_errors_status_1` will fail because they expect 1. The usage tests may or may not pass — verify each individually.

- [ ] **Step 3: Update `builtin_fg`**

Replace the top of `builtin_fg` (the arg-validation + id lookup section). The existing code:

```rust
fn builtin_fg(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if !args.is_empty() {
        eprintln!("shuck: fg: arguments not supported in this version");
        return ExecOutcome::Continue(2);
    }
    let id = match shell.jobs.current_id() {
        Some(id) => id,
        None => {
            eprintln!("shuck: fg: no current job");
            return ExecOutcome::Continue(1);
        }
    };
    ...
```

Becomes:

```rust
fn builtin_fg(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let id = match args.len() {
        0 => match shell.jobs.current_id() {
            Some(id) => id,
            None => {
                eprintln!("shuck: fg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => match resolve_spec_or_error(&args[0], "fg", shell) {
            Ok(id) => id,
            Err(outcome) => return outcome,
        },
        _ => {
            eprintln!("shuck: fg: usage: fg [%job]");
            return ExecOutcome::Continue(2);
        }
    };
    ...
```

Leave the rest of `builtin_fg` (the resume/wait/restore-terminal logic) unchanged.

- [ ] **Step 4: Run tests**

Run: `cargo test builtins::fg_bg_tests`
Expected: all green (the new tests plus the pre-existing ones, since no-arg behavior is preserved).

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: 232 passed (228 + 4 new).

- [ ] **Step 6: Commit**

```bash
git add src/builtins.rs
git commit -m "feat(fg): accept %spec job argument"
```

---

## Task 4: extend `builtin_bg` to accept `%spec`

**Files:**
- Modify: `src/builtins.rs::builtin_bg`
- Test: `src/builtins.rs::fg_bg_tests`

- [ ] **Step 1: Write failing tests**

Add to `src/builtins.rs::fg_bg_tests`:

```rust
#[test]
fn bg_with_bad_job_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("bg", &["%abc".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_with_no_such_job_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("bg", &["%99".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_with_running_spec_errors_already_running() {
    let mut shell = Shell::new();
    shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("bg", &["%1".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn bg_with_multiple_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "bg",
        &["%1".to_string(), "%2".to_string()],
        &mut buf,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}
```

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test builtins::fg_bg_tests::bg_with`
Expected: pre-existing tests still pass; new tests fail because current `bg` rejects any args with status 2.

- [ ] **Step 3: Update `builtin_bg`**

Replace the top of `builtin_bg`. Existing:

```rust
fn builtin_bg(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    if !args.is_empty() {
        eprintln!("shuck: bg: arguments not supported in this version");
        return ExecOutcome::Continue(2);
    }
    let id = match shell.jobs.current_stopped_id() {
        Some(id) => id,
        None => {
            eprintln!("shuck: bg: no current job");
            return ExecOutcome::Continue(1);
        }
    };
    ...
```

Becomes:

```rust
fn builtin_bg(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    let id = match args.len() {
        0 => match shell.jobs.current_stopped_id() {
            Some(id) => id,
            None => {
                eprintln!("shuck: bg: no current job");
                return ExecOutcome::Continue(1);
            }
        },
        1 if args[0].starts_with('%') => {
            let id = match resolve_spec_or_error(&args[0], "bg", shell) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            // Verify the resolved job is actually Stopped.
            let is_stopped = shell.jobs.iter()
                .find(|j| j.id == id)
                .map(|j| matches!(j.state, crate::jobs::JobState::Stopped(_)))
                .unwrap_or(false);
            if !is_stopped {
                eprintln!("shuck: bg: job %{id} already running");
                return ExecOutcome::Continue(1);
            }
            id
        }
        _ => {
            eprintln!("shuck: bg: usage: bg [%job]");
            return ExecOutcome::Continue(2);
        }
    };
    ...
```

Leave the rest of `builtin_bg` unchanged.

- [ ] **Step 4: Run tests**

Run: `cargo test builtins::fg_bg_tests`
Expected: all green.

- [ ] **Step 5: Run full suite**

Run: `cargo test`
Expected: 236 passed (232 + 4 new).

- [ ] **Step 6: Commit**

```bash
git add src/builtins.rs
git commit -m "feat(bg): accept %spec; reject when target is Running"
```

---

## Task 5: extend `builtin_wait` to accept `%spec` and bare PID

**Files:**
- Modify: `src/builtins.rs::builtin_wait`
- Test: `src/builtins.rs` (new tests near existing `wait` tests, or in `fg_bg_tests`)

- [ ] **Step 1: Write failing tests**

Add to `src/builtins.rs::fg_bg_tests`:

```rust
#[test]
fn wait_with_bad_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("wait", &["%abc".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn wait_with_no_such_spec_errors_status_1() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("wait", &["%99".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}

#[test]
fn wait_with_multiple_args_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin(
        "wait",
        &["%1".to_string(), "%2".to_string()],
        &mut buf,
        &mut shell,
    );
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn wait_with_unparseable_pid_arg_returns_usage_status_2() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("wait", &["abc".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(2)));
}

#[test]
fn wait_with_done_spec_returns_decoded_status_immediately() {
    let mut shell = Shell::new();
    // Synthetic Done job — wait should see it's already terminal and
    // return decode(0) → 0 without blocking.
    shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("wait", &["%1".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn wait_with_done_spec_returns_nonzero_for_exit_n() {
    let mut shell = Shell::new();
    shell.jobs.add_synthetic_done("false".to_string(), 1);
    let mut buf: Vec<u8> = Vec::new();
    let outcome = run_builtin("wait", &["%1".to_string()], &mut buf, &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(1)));
}
```

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test builtins::fg_bg_tests::wait_with`
Expected: tests fail because current `wait` rejects all args with status 2.

- [ ] **Step 3: Update `builtin_wait`**

Replace the entire body of `builtin_wait`. Current body:

```rust
fn builtin_wait(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    if !args.is_empty() {
        eprintln!("shuck: wait: arguments not supported in this version");
        return ExecOutcome::Continue(2);
    }
    while shell.jobs.has_pending() {
        if shell.sigint_flag
            .compare_exchange(true, false, std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            eprintln!();
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
    crate::jobs::reap_and_notify(shell);
    ExecOutcome::Continue(0)
}
```

Becomes:

```rust
fn builtin_wait(args: &[String], _out: &mut dyn std::io::Write, shell: &mut Shell) -> ExecOutcome {
    match args.len() {
        0 => wait_all(shell),
        1 if args[0].starts_with('%') => {
            let id = match resolve_spec_or_error(&args[0], "wait", shell) {
                Ok(id) => id,
                Err(outcome) => return outcome,
            };
            wait_for_job(id, shell)
        }
        1 => match args[0].parse::<i32>() {
            Ok(pid) if pid > 0 => wait_for_pid(pid, shell),
            _ => {
                eprintln!("shuck: wait: usage: wait [%job | pid]");
                ExecOutcome::Continue(2)
            }
        },
        _ => {
            eprintln!("shuck: wait: usage: wait [%job | pid]");
            ExecOutcome::Continue(2)
        }
    }
}

fn wait_all(shell: &mut Shell) -> ExecOutcome {
    while shell.jobs.has_pending() {
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    crate::jobs::reap_and_notify(shell);
    ExecOutcome::Continue(0)
}

fn wait_for_job(id: u32, shell: &mut Shell) -> ExecOutcome {
    loop {
        // Check terminal state first — handles already-Done jobs.
        let terminal = shell.jobs.iter()
            .find(|j| j.id == id)
            .and_then(|j| match j.state {
                crate::jobs::JobState::Done(c) => Some(c),
                crate::jobs::JobState::Signaled(s) => Some(128 + s),
                _ => None,
            });
        if let Some(code) = terminal {
            return ExecOutcome::Continue(code);
        }
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

fn wait_for_pid(pid: i32, shell: &mut Shell) -> ExecOutcome {
    let mut first = true;
    loop {
        if check_sigint(shell) { return ExecOutcome::Continue(130); }
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) };
        if r > 0 {
            shell.jobs.reap(r, status);
            if libc::WIFSTOPPED(status) {
                // Still alive; keep polling.
                first = false;
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let code = if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                1
            };
            return ExecOutcome::Continue(code);
        }
        if r < 0 {
            // ECHILD: not a child (or already reaped). On the first call,
            // surface as "not a child." On a subsequent call, treat as a
            // race we can't recover from.
            if first {
                eprintln!("shuck: wait: pid {pid} is not a child of this shell");
                return ExecOutcome::Continue(127);
            }
            return ExecOutcome::Continue(1);
        }
        first = false;
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn check_sigint(shell: &Shell) -> bool {
    if shell.sigint_flag
        .compare_exchange(
            true,
            false,
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
        )
        .is_ok()
    {
        eprintln!();
        true
    } else {
        false
    }
}
```

- [ ] **Step 4: Build, then run tests**

Run: `cargo build`
Expected: clean.

Run: `cargo test builtins::fg_bg_tests::wait_with`
Expected: all PASS.

Run: `cargo test`
Expected: 242 passed (236 + 6 new).

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs
git commit -m "feat(wait): accept %spec and bare PID; return decoded exit status"
```

---

## Task 6: `kill` builtin

**Files:**
- Modify: `src/builtins.rs` — `is_builtin`, `run_builtin` dispatch, new `builtin_kill`, signal table.
- Test: `src/builtins.rs`

- [ ] **Step 1: Write failing tests**

Add a new test module to `src/builtins.rs`:

```rust
#[cfg(test)]
mod kill_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn is_builtin_recognizes_kill() {
        assert!(is_builtin("kill"));
    }

    #[test]
    fn kill_no_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_sig_flag_with_no_targets_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-TERM".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn kill_invalid_signal_name_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-ABC".to_string(), "%1".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_invalid_signal_number_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-9999".to_string(), "%1".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_unparseable_target_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_no_such_job_spec_returns_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["%99".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn signal_by_name_table_recognizes_common_signals() {
        assert_eq!(signal_by_name("HUP"), Some(libc::SIGHUP));
        assert_eq!(signal_by_name("SIGHUP"), Some(libc::SIGHUP));
        assert_eq!(signal_by_name("hup"), Some(libc::SIGHUP));
        assert_eq!(signal_by_name("sighup"), Some(libc::SIGHUP));
        assert_eq!(signal_by_name("INT"), Some(libc::SIGINT));
        assert_eq!(signal_by_name("KILL"), Some(libc::SIGKILL));
        assert_eq!(signal_by_name("TERM"), Some(libc::SIGTERM));
        assert_eq!(signal_by_name("STOP"), Some(libc::SIGSTOP));
        assert_eq!(signal_by_name("CONT"), Some(libc::SIGCONT));
        assert_eq!(signal_by_name("USR1"), Some(libc::SIGUSR1));
        assert_eq!(signal_by_name("USR2"), Some(libc::SIGUSR2));
        assert_eq!(signal_by_name("ABC"), None);
        assert_eq!(signal_by_name(""), None);
    }
}
```

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test builtins::kill_tests`
Expected: FAIL — `kill` not in dispatch; `signal_by_name` doesn't exist.

- [ ] **Step 3: Add `signal_by_name`**

Add at module scope in `src/builtins.rs`, near the other helpers:

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

- [ ] **Step 4: Implement `builtin_kill`**

Add the function to `src/builtins.rs`:

```rust
fn builtin_kill(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let (sig, targets) = if let Some(first) = args.first() {
        if let Some(rest) = first.strip_prefix('-') {
            // -<sig> form
            let sig = match rest.parse::<i32>() {
                Ok(n) if (1..=64).contains(&n) => n,
                Ok(_) => {
                    eprintln!("shuck: kill: {rest}: invalid signal number");
                    return ExecOutcome::Continue(1);
                }
                Err(_) => match signal_by_name(rest) {
                    Some(n) => n,
                    None => {
                        eprintln!("shuck: kill: {rest}: invalid signal");
                        return ExecOutcome::Continue(1);
                    }
                },
            };
            if args.len() < 2 {
                eprintln!("shuck: kill: usage: kill [-sig] pid | %job ...");
                return ExecOutcome::Continue(2);
            }
            (sig, &args[1..])
        } else {
            (libc::SIGTERM, &args[..])
        }
    } else {
        eprintln!("shuck: kill: usage: kill [-sig] pid | %job ...");
        return ExecOutcome::Continue(2);
    };

    let mut any_failed = false;
    for target in targets {
        if let Some(_rest) = target.strip_prefix('%') {
            let id = match resolve_spec_or_error(target, "kill", shell) {
                Ok(id) => id,
                Err(_) => {
                    any_failed = true;
                    continue;
                }
            };
            let pgid = match shell.jobs.iter().find(|j| j.id == id) {
                Some(j) => j.pgid,
                None => {
                    eprintln!("shuck: kill: {target}: no such job");
                    any_failed = true;
                    continue;
                }
            };
            let rc = unsafe { libc::killpg(pgid, sig) };
            if rc != 0 {
                let errno = std::io::Error::last_os_error();
                eprintln!("shuck: kill: ({target}) - {errno}");
                any_failed = true;
            }
        } else {
            match target.parse::<i32>() {
                Ok(pid) if pid > 0 => {
                    let rc = unsafe { libc::kill(pid, sig) };
                    if rc != 0 {
                        let errno = std::io::Error::last_os_error();
                        eprintln!("shuck: kill: ({pid}) - {errno}");
                        any_failed = true;
                    }
                }
                _ => {
                    eprintln!("shuck: kill: {target}: arguments must be process or job IDs");
                    any_failed = true;
                }
            }
        }
    }

    if any_failed { ExecOutcome::Continue(1) } else { ExecOutcome::Continue(0) }
}
```

- [ ] **Step 5: Register `kill` in `is_builtin` and dispatch**

Add `"kill"` to the matcher in `is_builtin`. Add to `run_builtin`:

```rust
"kill" => builtin_kill(args, shell),
```

- [ ] **Step 6: Run tests**

Run: `cargo test builtins::kill_tests`
Expected: all 8 PASS.

Run: `cargo test`
Expected: 250 passed (242 + 8 new).

- [ ] **Step 7: Commit**

```bash
git add src/builtins.rs
git commit -m "feat: add kill builtin (-<sig> or default SIGTERM; %spec or PID targets)"
```

---

## Task 7: `disown` builtin

**Files:**
- Modify: `src/builtins.rs` — `is_builtin`, `run_builtin` dispatch, new `builtin_disown`.
- Test: `src/builtins.rs`

- [ ] **Step 1: Write failing tests**

Add to `src/builtins.rs`:

```rust
#[cfg(test)]
mod disown_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn is_builtin_recognizes_disown() {
        assert!(is_builtin("disown"));
    }

    #[test]
    fn disown_no_args_with_no_current_job_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_no_args_removes_current_job() {
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_with_spec_removes_specified_job() {
        let mut shell = Shell::new();
        shell.jobs.add(100, vec![100], "a".to_string());
        shell.jobs.add(200, vec![200], "b".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let remaining: Vec<u32> = shell.jobs.iter().map(|j| j.id).collect();
        assert_eq!(remaining, vec![2]);
    }

    #[test]
    fn disown_with_bad_spec_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["%abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn disown_with_non_percent_arg_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn disown_with_multiple_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "disown",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn disown_drops_pending_done_notification() {
        let mut shell = Shell::new();
        // Synthetic Done job with notified=false would trigger a "[1] Done"
        // line at the next prompt. Disown should remove the job and
        // suppress that notification.
        shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }
}
```

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test builtins::disown_tests`
Expected: FAIL — `disown` not recognized.

- [ ] **Step 3: Implement `builtin_disown`**

Add to `src/builtins.rs`:

```rust
fn builtin_disown(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("shuck: disown: usage: disown [%job]");
        return ExecOutcome::Continue(2);
    }
    let id = match args.first() {
        Some(arg) if arg.starts_with('%') => match resolve_spec_or_error(arg, "disown", shell) {
            Ok(id) => id,
            Err(outcome) => return outcome,
        },
        Some(_) => {
            eprintln!("shuck: disown: usage: disown [%job]");
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

- [ ] **Step 4: Register `disown` in `is_builtin` and dispatch**

Add `"disown"` to `is_builtin`. Add to `run_builtin`:

```rust
"disown" => builtin_disown(args, shell),
```

- [ ] **Step 5: Run tests**

Run: `cargo test builtins::disown_tests`
Expected: all 8 PASS.

Run: `cargo test`
Expected: 258 passed (250 + 8 new).

- [ ] **Step 6: Commit**

```bash
git add src/builtins.rs
git commit -m "feat: add disown builtin (no-arg or %spec)"
```

---

## Task 8: Smoke test against a real terminal

**Files:** none (manual verification only)

Job-spec resolution, signal delivery, disown effects — these are runtime behaviors best verified in a real shell session.

- [ ] **Step 1: Build the release binary**

Run: `cargo build --release`
Expected: clean build at `target/release/shuck`.

- [ ] **Step 2: Walk the smoke-test checklist**

Run `target/release/shuck` in a real terminal. Verify each:

1. **`kill %N` SIGTERMs a job.**
   - `sleep 100 &`
   - `kill %1`
   - Expected: at the next prompt, `[1]+ Killed (signal 15)         sleep 100 &`.

2. **`kill -STOP %N` and `bg %N` round-trip.**
   - `sleep 100 &`
   - `kill -STOP %1`
   - `jobs` → `[1]+ Stopped              sleep 100`.
   - `bg %1`
   - `jobs` → `[1]+ Running              sleep 100`.

3. **`kill -9 PID` works on a bare PID.**
   - `sleep 100 &`
   - Note the PID from the `[1] <pid>` line.
   - `kill -9 <pid>`
   - At next prompt: `[1]+ Killed (signal 9)`.

4. **`disown` removes the job, process keeps running.**
   - `sleep 60 &`
   - `disown %1`
   - `jobs` → empty.
   - In another terminal, `ps -ef | grep sleep` → still running.

5. **`wait %N` returns the right exit status.**
   - `sleep 2 &`
   - `wait %1; echo $?` → after 2s, prints `0`.

6. **`wait PID` works.**
   - `sleep 2 &`
   - Note the PID.
   - `wait <pid>; echo $?` → after 2s, prints `0`.

7. **`wait 99999` on a non-child returns 127 immediately.**
   - `wait 99999; echo $?` → prints error, then `127`.

8. **`fg %N` resumes a stopped pipeline.**
   - `sleep 100 &`
   - `kill -STOP %1`
   - `fg %1`
   - Press **Ctrl-Z** → returns to prompt with `[1]+ Stopped`.

9. **`bg %N` on a Running job errors.**
   - `sleep 100 &`
   - `bg %1`
   - Expected: `shuck: bg: job %1 already running`, status 1.

10. **`%-` resolves to previous job.**
    - `sleep 100 &`
    - `sleep 200 &`
    - `kill %-`
    - Expected: job 1 (the older one) killed.

11. **Bad spec gives a clean error.**
    - `kill %abc` → `shuck: kill: %abc: bad job spec`, status 1.
    - `fg %99` → `shuck: fg: %99: no such job`, status 1.

- [ ] **Step 3: Note any failures and fix before merge**

If anything fails, file a fix task and address it before the wrap-up. Do not commit anything in this task — verification only.

---

## Wrap-up

- [ ] **Final cross-branch code review**

Dispatch `feature-dev:code-reviewer` over the full diff:

```bash
git diff main...HEAD
```

Focus areas:
- Job-spec parsing correctness (especially the `%-1` / `%1x` edge cases).
- `JobTable::resolve` consistency with `current_id` / `current_and_previous`.
- `kill` signal-target dispatch (pgrp vs pid, error formatting).
- `wait %spec` correctly drains terminal-state jobs without blocking.
- `wait PID` distinguishes "not a child" (first call) from race-after-reap.
- `disown` doesn't accidentally orphan jobs the user might still expect.

- [ ] **Apply review fixes; merge to main**

Match the v6/v7 merge flow: `--no-ff` merge, delete the branch, push.

---

## Self-review notes

**Spec coverage:**
- §1 (JobSpec parser + resolver) → Tasks 1, 2.
- §2 (`kill`) → Task 6.
- §3 (`disown`) → Task 7.
- §4 (extend fg/bg/wait) → Tasks 3, 4, 5.
- §5 (edge cases) → covered via test cases in each builtin task + smoke test items.
- §6 (file summary) → matches Tasks 1–7 file lists.
- §7 (testing) → unit tests in Tasks 1–7, manual smoke in Task 8.
- §8 (out of scope) → no implementation; deferral documented.

**Placeholder scan:** No TBD/TODO/"similar to" steps. Every code step has the complete code.

**Type consistency:**
- `JobSpec` and `JobSpecError` defined in Task 1; used by Task 2's `resolve` and `resolve_spec_or_error`; used by Tasks 3–7 via `resolve_spec_or_error`.
- `resolve_spec_or_error(arg, builtin, shell) -> Result<u32, ExecOutcome>` defined in Task 2; used unchanged by Tasks 3, 4, 5, 6, 7.
- `signal_by_name(s) -> Option<i32>` defined in Task 6; only used by `builtin_kill`.

**Test-file growth note:** `src/builtins.rs` already has `fg_bg_tests`. Tasks 3–5 add to it; Tasks 6 and 7 add new `kill_tests` and `disown_tests` modules. Acceptable — keeps test code close to the function under test.
