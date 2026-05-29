# huck v43 — `disown -a/-r/-h` + SIGHUP-on-exit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close bash divergence M-43 by adding `-a`/`-r`/`-h` flag
support and multi-arg support to `disown`, plus the SIGHUP-on-exit
behavior that gives `-h` real semantics.

**Architecture:** Three files change. `src/jobs.rs` gains a
`marked_for_nohup: bool` field on `Job` + `mark_for_nohup` helper on
`JobTable`. `src/shell_state.rs` gains a `Shell::hangup_jobs()`
method (and a private `should_hangup` pure helper).
`src/shell.rs::run` calls `shell.hangup_jobs()` on each of the four
clean-exit paths. `src/builtins.rs::builtin_disown` is rewritten as
a flag parser + multi-arg job-set selector.

**Tech Stack:** Rust. `libc::killpg` for SIGCONT/SIGHUP delivery.

**Spec:** `docs/superpowers/specs/2026-05-28-huck-disown-flags-design.md`

**Branch:** `v43-disown-flags` (created in preamble step P.1).

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
git checkout -b v43-disown-flags
```

Expected: `Switched to a new branch 'v43-disown-flags'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: Foundation + builtin rewrite + unit tests

**Files:**
- Modify: `src/jobs.rs` — add `marked_for_nohup: bool` field on `Job`; initialize in `JobTable::add` and `JobTable::add_synthetic_done`; add `mark_for_nohup` helper.
- Modify: `src/shell_state.rs` — add `Shell::hangup_jobs()` + private `should_hangup` helper; one unit test in the existing `#[cfg(test)] mod tests` block.
- Modify: `src/shell.rs` — call `shell.hangup_jobs()` at all four clean-exit return sites in `run`.
- Modify: `src/builtins.rs` — replace `builtin_disown` body with flag parser + multi-arg dispatch; append 9 unit tests in `mod disown_tests`.

### Step 1.1: Add `marked_for_nohup` field to `Job`

In `src/jobs.rs:19-29`, find the `Job` struct:

```rust
#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    #[allow(dead_code)]
    pub pgid: i32,
    pub pids: Vec<i32>,
    pub reaped: Vec<bool>,
    pub last_status: Option<i32>,
    pub command: String,
    pub state: JobState,
    pub notified: bool,
    pub created_at: u64,
}
```

Add a new field at the end:

```rust
#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    #[allow(dead_code)]
    pub pgid: i32,
    pub pids: Vec<i32>,
    pub reaped: Vec<bool>,
    pub last_status: Option<i32>,
    pub command: String,
    pub state: JobState,
    pub notified: bool,
    pub created_at: u64,
    pub marked_for_nohup: bool,
}
```

### Step 1.2: Initialize the field in `JobTable::add`

In `src/jobs.rs:45-66`, find the `pub fn add` body. After the existing field initializations inside `let job = Job { ... };`, add `marked_for_nohup: false,`:

The full updated function:

```rust
pub fn add(&mut self, pgid: i32, pids: Vec<i32>, command: String) -> u32 {
    let id = self.next_id();
    let n = pids.len();
    let job = Job {
        id,
        pgid,
        pids,
        reaped: vec![false; n],
        last_status: None,
        command,
        state: JobState::Running,
        notified: false,
        created_at: self.next_created_at,
        marked_for_nohup: false,
    };
    self.next_created_at += 1;
    self.jobs.push(job);
    self.jobs.sort_by_key(|j| j.id);
    id
}
```

### Step 1.3: Initialize the field in `JobTable::add_synthetic_done`

In `src/jobs.rs:67-90`, find `pub fn add_synthetic_done`. Add `marked_for_nohup: false,` at the end of its `Job { ... }` literal. The updated function:

```rust
pub fn add_synthetic_done(&mut self, command: String, exit: i32) -> u32 {
    let id = self.next_id();
    let job = Job {
        id,
        pgid: 0,
        pids: Vec::new(),
        reaped: Vec::new(),
        last_status: Some(exit << 8),
        command,
        state: JobState::Done(exit),
        notified: false,
        created_at: self.next_created_at,
        marked_for_nohup: false,
    };
    self.next_created_at += 1;
    self.jobs.push(job);
    self.jobs.sort_by_key(|j| j.id);
    id
}
```

### Step 1.4: Add `JobTable::mark_for_nohup`

Still in `src/jobs.rs`, find the `pub fn jobs_mut` definition at line 205. **Immediately above** it (or right after `current_id` at line 167 — your call, whichever keeps the file readable), add:

```rust
    /// Marks the job with id `id` as exempt from the shell's
    /// SIGHUP-on-exit broadcast. No-op if the id doesn't exist.
    pub fn mark_for_nohup(&mut self, id: u32) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.marked_for_nohup = true;
        }
    }
```

### Step 1.5: Build to confirm `src/jobs.rs` compiles

Run: `cargo build`
Expected: clean. The field addition cascades into every existing
`Job { ... }` literal — if there are any in the codebase outside
`src/jobs.rs`, the build will tell you. There shouldn't be any.

### Step 1.6: Add `should_hangup` + `Shell::hangup_jobs` in `src/shell_state.rs`

In `src/shell_state.rs`, find the `impl Shell { ... }` block (starts at line 83). The block ends somewhere around line 220+ (before the `#[cfg(test)] mod tests` block at line 229). **Immediately before** the closing `}` of the `impl Shell` block (search for the `}` that closes `impl Shell`), add:

```rust
    /// Sends SIGHUP to every live job not marked for nohup. Called
    /// on each clean shell-exit path. Stopped jobs get SIGCONT first
    /// so they wake to die. Errors from `killpg` (e.g. ESRCH for an
    /// already-reaped pgid) are intentionally ignored; this is a
    /// best-effort cleanup.
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
```

Then **outside** the `impl Shell` block (immediately after it closes), add the pure helper:

```rust
/// Pure predicate: should this job receive SIGHUP at shell exit?
/// True iff the job is still alive (Running or Stopped) AND has
/// not been marked for nohup by `disown -h`.
fn should_hangup(job: &crate::jobs::Job) -> bool {
    let live = matches!(
        job.state,
        crate::jobs::JobState::Running | crate::jobs::JobState::Stopped(_)
    );
    live && !job.marked_for_nohup
}
```

### Step 1.7: Add the `should_hangup` unit test

In `src/shell_state.rs`, find the `#[cfg(test)] mod tests { ... }` block at line 229. Append this test inside it, before its closing `}`:

```rust
    #[test]
    fn should_hangup_skips_marked_and_done_jobs() {
        use crate::jobs::{JobState, JobTable};
        let mut t = JobTable::new();
        let id = t.add(0, vec![1234], "sleep 30".to_string());

        // Running + not marked → hangup
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(super::should_hangup(job));

        // Running + marked → skip
        t.mark_for_nohup(id);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(!super::should_hangup(job));

        // Done + not marked → skip
        t.jobs_mut()[0].marked_for_nohup = false;
        t.jobs_mut()[0].state = JobState::Done(0);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(!super::should_hangup(job));
    }
```

### Step 1.8: Run the new test to confirm it passes

Run: `cargo test should_hangup_ -- --nocapture`
Expected: 1 test passes.

### Step 1.9: Hook `hangup_jobs` into the four clean-exit sites in `src/shell.rs`

In `src/shell.rs::run`, there are four return sites that exit cleanly. Add a `shell.hangup_jobs();` call IMMEDIATELY before `shell.history.save();` at each.

**Site 1** (`ExecOutcome::Exit`, currently line 72-76):

Before:
```rust
                    ExecOutcome::Exit(code) => {
                        crate::traps::fire_exit_trap(&mut shell);
                        shell.history.save();
                        return code;
                    }
```

After:
```rust
                    ExecOutcome::Exit(code) => {
                        crate::traps::fire_exit_trap(&mut shell);
                        shell.hangup_jobs();
                        shell.history.save();
                        return code;
                    }
```

**Site 2** (fatal PE error non-interactive, currently line 86-91):

Before:
```rust
                        if let Some(fatal_status) = shell.take_pending_fatal_pe_error()
                            && !shell.is_interactive
                        {
                            crate::traps::fire_exit_trap(&mut shell);
                            shell.history.save();
                            return fatal_status;
                        }
```

After:
```rust
                        if let Some(fatal_status) = shell.take_pending_fatal_pe_error()
                            && !shell.is_interactive
                        {
                            crate::traps::fire_exit_trap(&mut shell);
                            shell.hangup_jobs();
                            shell.history.save();
                            return fatal_status;
                        }
```

**Site 3** (`ReadResult::Eof`, currently line 99-103):

Before:
```rust
            ReadResult::Eof => {
                crate::traps::fire_exit_trap(&mut shell);
                shell.history.save();
                return shell.last_status();
            }
```

After:
```rust
            ReadResult::Eof => {
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.history.save();
                return shell.last_status();
            }
```

**Site 4** (`ReadResult::EofMidCommand`, currently line 105-110):

Before:
```rust
            ReadResult::EofMidCommand => {
                eprintln!("huck: syntax error: unexpected end of input");
                crate::traps::fire_exit_trap(&mut shell);
                shell.history.save();
                return 2;
            }
```

After:
```rust
            ReadResult::EofMidCommand => {
                eprintln!("huck: syntax error: unexpected end of input");
                crate::traps::fire_exit_trap(&mut shell);
                shell.hangup_jobs();
                shell.history.save();
                return 2;
            }
```

**Site 5** (`ReadResult::ReadError`, currently around line 111-118): scan downward from line 110 to find the `ReadResult::ReadError(msg) => { ... return X; }` arm. Apply the same pattern (insert `shell.hangup_jobs();` immediately before `shell.history.save();`).

### Step 1.10: Build

Run: `cargo build`
Expected: clean.

### Step 1.11: Run all tests to confirm no regression

Run: `cargo test --bin huck`
Expected: all unit tests pass. The field addition shouldn't break
anything; the `hangup_jobs` calls don't fire in unit tests because
`run` isn't exercised by `cargo test`.

### Step 1.12: Rewrite `builtin_disown`

In `src/builtins.rs`, find `fn builtin_disown` (starts at line 956). Replace its entire body with:

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
                break;
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
        // bash: positional args are ignored when -a is supplied.
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
        shell
            .jobs
            .jobs_mut()
            .retain(|j| !target_ids.contains(&j.id));
    }

    ExecOutcome::Continue(0)
}
```

### Step 1.13: Build

Run: `cargo build`
Expected: clean.

### Step 1.14: Add the 9 disown unit tests

Find `mod disown_tests` in `src/builtins.rs` (starts at line 2247: `#[cfg(test)] mod disown_tests { use super::*; use crate::shell_state::Shell; ... }`). The mod already has `use super::*;` and `use crate::shell_state::Shell;` so `run_builtin`, `ExecOutcome`, and `Shell` are in scope. The pre-existing tests in this block were written assuming single-arg semantics; **read them first** to identify any that no longer hold under the new flag-aware dispatcher. The most likely casualty is a test asserting that `disown %1 %2` returns a usage error — if found, delete or repurpose it.

Append these 9 new tests inside the same mod block, before its closing `}`:

```rust
    use crate::jobs::JobState;

    #[test]
    fn disown_a_removes_all_jobs() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("a".to_string(), 0);
        shell.jobs.add_synthetic_done("b".to_string(), 0);
        shell.jobs.add_synthetic_done("c".to_string(), 0);
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-a".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }

    #[test]
    fn disown_r_filters_to_running_only() {
        let mut shell = Shell::new();
        // %1 Running (real add), %2 Done, %3 Done
        shell.jobs.add(1234, vec![1234], "sleep".to_string()); // %1 Running
        shell.jobs.add_synthetic_done("a".to_string(), 0);     // %2 Done
        shell.jobs.add_synthetic_done("b".to_string(), 0);     // %3 Done
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-r".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // %1 (Running) was removed; %2 and %3 remain.
        assert_eq!(shell.jobs.iter().count(), 2);
        assert!(shell.jobs.iter().all(|j| matches!(j.state, JobState::Done(_))));
    }

    #[test]
    fn disown_h_marks_for_nohup_keeps_in_table() {
        let mut shell = Shell::new();
        let id = shell.jobs.add(1234, vec![1234], "sleep".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "disown",
            &["-h".to_string(), "%1".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let job = shell.jobs.iter().find(|j| j.id == id).expect("job removed!");
        assert!(job.marked_for_nohup);
    }

    #[test]
    fn disown_multiple_args_processes_each() {
        let mut shell = Shell::new();
        shell.jobs.add_synthetic_done("a".to_string(), 0); // %1
        shell.jobs.add_synthetic_done("b".to_string(), 0); // %2
        shell.jobs.add_synthetic_done("c".to_string(), 0); // %3
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "disown",
            &["%1".to_string(), "%2".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // Only %3 should remain.
        let ids: Vec<u32> = shell.jobs.iter().map(|j| j.id).collect();
        assert_eq!(ids, vec![3]);
    }

    #[test]
    fn disown_ah_marks_all() {
        let mut shell = Shell::new();
        let id1 = shell.jobs.add(1234, vec![1234], "a".to_string());
        let id2 = shell.jobs.add(1235, vec![1235], "b".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-ah".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // Both still in table; both marked.
        assert_eq!(shell.jobs.iter().count(), 2);
        assert!(shell.jobs.iter().find(|j| j.id == id1).unwrap().marked_for_nohup);
        assert!(shell.jobs.iter().find(|j| j.id == id2).unwrap().marked_for_nohup);
    }

    #[test]
    fn disown_ar_removes_all_running() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string()); // %1 Running
        shell.jobs.add(1235, vec![1235], "b".to_string()); // %2 Running
        shell.jobs.add_synthetic_done("c".to_string(), 0); // %3 Done
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-ar".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // Only %3 (Done) should remain.
        let states: Vec<&JobState> = shell.jobs.iter().map(|j| &j.state).collect();
        assert_eq!(states.len(), 1);
        assert!(matches!(states[0], JobState::Done(_)));
    }

    #[test]
    fn disown_arh_marks_all_running() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string()); // %1 Running
        shell.jobs.add(1235, vec![1235], "b".to_string()); // %2 Running
        shell.jobs.add_synthetic_done("c".to_string(), 0); // %3 Done
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-arh".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // All three still present; only the two Running ones marked.
        assert_eq!(shell.jobs.iter().count(), 3);
        for job in shell.jobs.iter() {
            match job.state {
                JobState::Running => assert!(job.marked_for_nohup, "running job not marked"),
                _ => assert!(!job.marked_for_nohup, "non-running job got marked"),
            }
        }
    }

    #[test]
    fn disown_invalid_flag_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("disown", &["-x".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn disown_a_ignores_positional_args() {
        let mut shell = Shell::new();
        shell.jobs.add(1234, vec![1234], "a".to_string()); // %1
        shell.jobs.add(1235, vec![1235], "b".to_string()); // %2
        shell.jobs.add(1236, vec![1236], "c".to_string()); // %3
        let mut buf: Vec<u8> = Vec::new();
        // bash-faithful: -a removes all, positional %1 is ignored.
        let outcome = run_builtin(
            "disown",
            &["-a".to_string(), "%1".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.jobs.iter().count(), 0);
    }
```

### Step 1.15: Run the new disown tests

Run: `cargo test disown_ should_hangup_ -- --nocapture`
Expected: 10 tests pass (9 disown + 1 should_hangup from step 1.7).

### Step 1.16: Full unit suite + clippy

Run: `cargo test --bin huck`
Expected: all unit tests pass.

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

### Step 1.17: Commit

```bash
git add src/jobs.rs src/shell_state.rs src/shell.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: disown -a/-r/-h + SIGHUP-on-exit (v43 task 1)

Foundation: add `marked_for_nohup: bool` field on Job in
src/jobs.rs (initialized false in add and add_synthetic_done);
new JobTable::mark_for_nohup(id) helper. Add Shell::hangup_jobs()
in src/shell_state.rs that iterates jobs and sends SIGCONT then
SIGHUP to each live job whose marked_for_nohup is false, via a
pure should_hangup(job) predicate. Hook shell.hangup_jobs() into
each of the four (sometimes five) clean-exit return sites in
src/shell.rs::run, immediately before history.save.

Behavior change: huck now sends SIGHUP to background jobs on
clean shell exit. Was: never sent. This matches bash's typical
interactive default. Defensive patterns (disown -h, nohup)
continue to work.

Rewrite builtin_disown as a flag parser + multi-arg dispatcher:
- Combined flag forms accepted: -a, -r, -h, -ah, -ar, -arh.
- -a: all jobs; positional args ignored (bash-faithful).
- -r: filter to Running state only.
- -h: mark each target for nohup instead of removing.
- No flags + no args: current job (existing behavior).
- No flags + positional %specs: each resolved sequentially.

Errors:
- `disown -x` (unknown flag) → status 2 + usage line.
- `disown foo` (no %) → status 1.
- `disown` with no current job → status 1.
- `disown %99` (no such job) → status 1 (via resolve_spec_or_error).

10 new unit tests: 9 disown dispatch paths covering -a, -r, -h,
multi-arg, -ah, -ar, -arh, invalid flag, -a-ignores-positional;
plus one pure should_hangup predicate test in src/shell_state.rs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Integration tests

**Files:**
- Create: `tests/disown_h_integration.rs`

Three binary-driven tests that verify SIGHUP delivery via real
process liveness probes.

### Step 2.1: Create the integration test file

Create `tests/disown_h_integration.rs` with this content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

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

/// True if a process with this pid is alive AND signalable from
/// the test process (`libc::kill(pid, 0)` returns 0).
fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Best-effort cleanup: send SIGTERM to a PID that may or may not
/// still be alive.
fn cleanup_kill(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

/// Parses the first integer from a string. Used to pull a PID out
/// of `jobs -p` output.
fn first_pid(s: &str) -> Option<i32> {
    for word in s.split_whitespace() {
        if let Ok(n) = word.parse::<i32>() {
            if n > 0 {
                return Some(n);
            }
        }
    }
    None
}

#[test]
fn disown_h_lets_bg_job_survive() {
    // sleep 30 & ; jobs -p ; disown -h %1 ; exit
    let script = "sleep 30 &\njobs -p\ndisown -h %1\nexit\n";
    let (out, _) = run_capture(script);
    let pid = first_pid(&out).expect(&format!("no pid found in: {:?}", out));
    // After huck exits, allow a brief moment for any SIGHUP to land.
    thread::sleep(Duration::from_millis(200));
    let alive = pid_alive(pid);
    cleanup_kill(pid);
    assert!(alive, "bg job (pid {pid}) was killed despite disown -h");
}

#[test]
fn disown_without_h_kills_bg_job_on_exit() {
    let script = "sleep 30 &\njobs -p\nexit\n";
    let (out, _) = run_capture(script);
    let pid = first_pid(&out).expect(&format!("no pid found in: {:?}", out));
    thread::sleep(Duration::from_millis(200));
    let alive = pid_alive(pid);
    if alive {
        cleanup_kill(pid);
        panic!("bg job (pid {pid}) survived shell exit; expected SIGHUP delivery");
    }
}

#[test]
fn disown_a_h_marks_all_alive() {
    let script = "sleep 30 &\nsleep 30 &\njobs -p\ndisown -ah\nexit\n";
    let (out, _) = run_capture(script);
    let pids: Vec<i32> = out
        .split_whitespace()
        .filter_map(|w| w.parse::<i32>().ok())
        .filter(|n| *n > 0)
        .collect();
    assert!(pids.len() >= 2, "expected ≥2 pids in stdout, got {:?}", pids);
    thread::sleep(Duration::from_millis(200));
    let all_alive: Vec<(i32, bool)> = pids.iter().map(|p| (*p, pid_alive(*p))).collect();
    // Cleanup regardless of assertion outcome.
    for &(pid, _) in &all_alive {
        cleanup_kill(pid);
    }
    for (pid, alive) in &all_alive {
        assert!(alive, "bg job (pid {pid}) was killed despite disown -ah");
    }
}
```

### Step 2.2: Run the new integration suite

Run: `cargo test --test disown_h_integration -- --nocapture`
Expected: all 3 tests pass.

If `disown_without_h_kills_bg_job_on_exit` fails because the bg
process is still alive after 200ms: bump the sleep to 500ms. If
`disown_h_lets_bg_job_survive` fails because the bg process is dead:
that's a real bug in Task 1 — re-investigate the hook sites in
`src/shell.rs`.

### Step 2.3: Run the full integration suite for regression

Run: `cargo test --tests`
Expected: all integration tests pass. Known PTY flake
`pty_compound_stage_pipeline_stops_and_resumes` may flake under
load; re-run in isolation if hit.

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

### Step 2.5: Commit

```bash
git add tests/disown_h_integration.rs
git commit -m "$(cat <<'EOF'
test: disown -h integration coverage (v43 task 2)

Three binary-driven tests verifying real SIGHUP behavior at shell
exit. `disown -h %1` followed by exit leaves the background sleep
process alive (probed via libc::kill(pid, 0)). `disown` omitted
leaves the bg sleep dead. `disown -ah` followed by exit leaves
multiple bg sleeps alive. Each test cleans up surviving sleeps
with SIGTERM.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — flip M-43, change-log entry.
- Modify: `README.md` — v43 row + clean stale entry from the "Not yet implemented" stanza.

### Step 3.1: Flip M-43 in `docs/bash-divergences.md`

Find the M-43 entry in the Job-control section. Currently:

```markdown
- **M-43: `disown -a`/`-r`/`-h`** — `[deferred]` medium. huck: only one bare-`%spec` arg. bash: flags + multiple args.
```

Replace with:

```markdown
- **M-43: `disown -a`/`-r`/`-h` (and multi-arg)** — `[fixed v43]` medium. All three flags supported with combined forms (`-ah`, `-ar`, `-arh`). `-a` operates on all jobs (positional args ignored, bash-faithful); `-r` filters to Running only; `-h` marks the job for nohup (skipped by the shell's SIGHUP-on-exit broadcast) instead of removing it. Multi-arg `disown %1 %2 %3` now valid. Adds SIGHUP-on-exit behavior: clean exit (explicit `exit`, EOF, fatal-PE, ReadError) now sends SIGCONT + SIGHUP to every live unmarked job via `Shell::hangup_jobs`. **Behavior change**: previously huck never sent SIGHUP on exit; v43 aligns with bash's interactive default. Defensive patterns (`disown -h`, `nohup`) continue to work.
```

### Step 3.2: Add v43 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most recent `**2026-05-28**` entry (v42, M-40). Add IMMEDIATELY after it:

```markdown
- **2026-05-28**: M-43 (`disown -a`/`-r`/`-h` + multi-arg) shipped as v43. New `Job.marked_for_nohup` field + `JobTable::mark_for_nohup` helper + `Shell::hangup_jobs` method + pure `should_hangup` predicate. `builtin_disown` rewritten as a flag parser + multi-arg dispatcher with combined-flag support (`-ah`, `-ar`, `-arh`). The four clean-exit sites in `src/shell.rs::run` now call `shell.hangup_jobs()` before `history.save()`. **Behavior change**: bg jobs now receive SIGHUP on clean shell exit (was: never sent); scripts relying on huck's old "always survives" behavior need to add `disown -h`. No new L-* divergences.
```

### Step 3.3: Add v43 row to README

In `README.md`, find the version table. After the v42 row (search for `| v42       |`), add:

```markdown
| v43       | `disown -a`/`-r`/`-h` + SIGHUP-on-exit (M-43)                  |
```

Final block:

```markdown
| v41       | `kill -l` (M-39) + README cleanup                              |
| v42       | `kill -s SIGNAME` + `kill -n SIGNUM` (M-40)                    |
| v43       | `disown -a`/`-r`/`-h` + SIGHUP-on-exit (M-43)                  |
```

Match the column padding of v41/v42 (count spaces in the actual file before the closing `|`).

### Step 3.4: Trim `disown -a`/`-r`/`-h` from the "Not yet implemented" stanza

In `README.md`, find the block at lines ~233-238:

```markdown
**Not yet implemented:**
brace expansion (`{a,b,c}`), extended job specs
(`%cmd`/`%?cmd`), `disown -a`/`-r`/`-h`,
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`), aliases.
```

Replace with:

```markdown
**Not yet implemented:**
brace expansion (`{a,b,c}`), extended job specs
(`%cmd`/`%?cmd`), backgrounded multi-pipeline sequences
(`cmd1 && cmd2 &`), aliases.
```

Removed: `disown -a`/`-r`/`-h` (shipped this iteration). Kept: brace expansion, extended job specs, backgrounded multi-pipelines, aliases.

### Step 3.5: Run the full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo known PTY flake).

### Step 3.6: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

### Step 3.7: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-43 fixed; v43 in README; trim stale entry

Job-control section: M-43 (`disown -a`/`-r`/`-h` + multi-arg)
flipped from [deferred] to [fixed v43] with descriptive text
covering all three flags, combined forms, the -a-ignores-positional
rule, and the SIGHUP-on-exit behavior change.

Change log: 2026-05-28 v43 entry summarizing the marked_for_nohup
field, hangup_jobs method, four clean-exit hook sites, and the
behavior change.

README: v43 row added to the version table; "Not yet implemented"
paragraph trimmed to remove `disown -a`/`-r`/`-h` (shipped this
iteration).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble, task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the
   full diff (`main..v43-disown-flags`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v43.
