# Sub-project B (fg, bg, Ctrl-Z) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add fg/bg/Ctrl-Z to shuck: foreground pipelines run in their own process group, Ctrl-Z transitions them to `Stopped`, and `fg`/`bg` builtins resume them.

**Architecture:** Mirror v6's background-pipeline pgrp setup on the foreground path, plus `tcsetpgrp` handoff to give children the controlling terminal. The SIGCHLD reaper switches to `WNOHANG | WUNTRACED` to detect stops. The shell installs `SIG_IGN` for `SIGTSTP`/`SIGTTIN`/`SIGTTOU` so it never suspends itself.

**Tech Stack:** Rust 2024 edition, `libc` for `tcsetpgrp`/`waitpid`/`killpg`/`getpgrp`/`signal`/`WUNTRACED`/`WIFSTOPPED`/`WSTOPSIG`/`SIGCONT`/`SIGTSTP`/`SIGTTIN`/`SIGTTOU`/`SIG_IGN`. No new dependencies.

**Branch:** `feature/job-control-b` off `main`.

---

## Pre-flight

- [ ] **Step 0a: Create the feature branch from main**

```bash
git checkout main
git pull --ff-only
git checkout -b feature/job-control-b
```

- [ ] **Step 0b: Baseline — confirm clean build and all tests pass**

Run: `cargo build && cargo test`
Expected: clean build, `test result: ok. 186 passed; 0 failed`.

---

## Task 1: `JobState::Stopped` + render + notification format

**Files:**
- Modify: `src/jobs.rs:10-15` (`JobState` enum), `src/jobs.rs:199-206` (`render_state`), `src/jobs.rs:181-197` (`reap_and_notify`), `src/jobs.rs:209-220` (`decode_status`)
- Test: `src/jobs.rs` (in-module `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add at the bottom of `src/jobs.rs` inside `mod tests`:

```rust
#[test]
fn render_state_stopped_sigtstp_is_plain_stopped() {
    assert_eq!(render_state(&JobState::Stopped(libc::SIGTSTP)), "Stopped");
}

#[test]
fn render_state_stopped_sigttin_includes_tty_input() {
    assert_eq!(
        render_state(&JobState::Stopped(libc::SIGTTIN)),
        "Stopped (tty input)"
    );
}

#[test]
fn render_state_stopped_sigttou_includes_tty_output() {
    assert_eq!(
        render_state(&JobState::Stopped(libc::SIGTTOU)),
        "Stopped (tty output)"
    );
}

#[test]
fn render_state_stopped_unknown_signal_falls_back_to_numeric() {
    assert_eq!(
        render_state(&JobState::Stopped(99)),
        "Stopped (signal 99)"
    );
}
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test --lib jobs::tests::render_state_stopped`
Expected: FAIL — `Stopped` variant does not exist on `JobState`.

- [ ] **Step 3: Add the `Stopped` variant**

Replace `src/jobs.rs:10-15`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped(i32),
    Done(i32),
    Signaled(i32),
}
```

- [ ] **Step 4: Extend `render_state`**

Replace `src/jobs.rs:199-206`:

```rust
pub fn render_state(state: &JobState) -> String {
    match state {
        JobState::Running => "Running".to_string(),
        JobState::Stopped(s) => match *s {
            libc::SIGTSTP => "Stopped".to_string(),
            libc::SIGTTIN => "Stopped (tty input)".to_string(),
            libc::SIGTTOU => "Stopped (tty output)".to_string(),
            n => format!("Stopped (signal {n})"),
        },
        JobState::Done(0) => "Done".to_string(),
        JobState::Done(n) => format!("Exit {n}"),
        JobState::Signaled(s) => format!("Killed (signal {s})"),
    }
}
```

- [ ] **Step 5: Run render_state tests — should pass**

Run: `cargo test --lib jobs::tests::render_state`
Expected: PASS (4 new tests + any pre-existing render_state tests).

- [ ] **Step 6: Add a runtime accessor on `JobTable` and a pure formatter**

In `src/jobs.rs`, add (not test-gated):

```rust
impl JobTable {
    pub fn jobs_mut(&mut self) -> &mut Vec<Job> {
        &mut self.jobs
    }
}
```

And add a pure formatter function near `render_state`:

```rust
/// Renders one notification line. The trailing `&` is included only for
/// Done/Signaled jobs — Stopped jobs are not "running in the background"
/// so the suffix would be misleading. Column width is 24 to fit
/// `Stopped (tty output)`.
pub fn notification_line(job: &Job, flag: char) -> String {
    let state = render_state(&job.state);
    let suffix = match job.state {
        JobState::Stopped(_) => "",
        _ => " &",
    };
    format!("[{}]{} {:<24} {}{}", job.id, flag, state, job.command, suffix)
}
```

- [ ] **Step 7: Write the failing test for notification format**

Add inside `mod tests`:

```rust
#[test]
fn notification_line_for_stopped_omits_ampersand() {
    let mut t = JobTable::new();
    t.add(4242, vec![4242], "sleep 100".to_string());
    t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
    let line = notification_line(&t.jobs_mut()[0], '+');
    assert_eq!(line, "[1]+ Stopped                  sleep 100");
}

#[test]
fn notification_line_for_done_includes_ampersand() {
    let mut t = JobTable::new();
    t.add_synthetic_done("echo hi".to_string(), 0);
    let line = notification_line(&t.jobs_mut()[0], ' ');
    assert_eq!(line, "[1]  Done                     echo hi &");
}

#[test]
fn notification_line_for_stopped_tty_input_shows_reason() {
    let mut t = JobTable::new();
    t.add(4242, vec![4242], "cat".to_string());
    t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTTIN);
    let line = notification_line(&t.jobs_mut()[0], '+');
    assert_eq!(line, "[1]+ Stopped (tty input)      cat");
}
```

(The exact spacing comes from `{:<24}` — the implementer should run the test once after step 7 and copy the actual rendered string into the assertion if column width math differs from my count.)

- [ ] **Step 8: Run the format tests — confirm they fail**

Run: `cargo test --lib jobs::tests::notification_line`
Expected: FAIL — `notification_line` function doesn't exist yet *(if step 6 already added it)*, OR PASS *(if step 6 added it)*. If they pass already, that's fine — proceed to step 9. The point of step 7 is to lock in the format before the next step that touches `reap_and_notify`.

- [ ] **Step 9: Update `reap_and_notify` to call `notification_line`**

Replace the inner loop in `src/jobs.rs:185-195` (inside `reap_and_notify`):

```rust
for job in notifs {
    let flag = if Some(job.id) == current {
        '+'
    } else if Some(job.id) == previous {
        '-'
    } else {
        ' '
    };
    eprintln!("{}", notification_line(&job, flag));
}
```

- [ ] **Step 10: Update `drain_notifications` to also surface Stopped**

Replace `src/jobs.rs:123-133` (inside `drain_notifications`):

```rust
pub fn drain_notifications(&mut self) -> Vec<Job> {
    let mut out = Vec::new();
    for job in self.jobs.iter_mut() {
        let pending = !matches!(job.state, JobState::Running);
        if pending && !job.notified {
            job.notified = true;
            out.push(job.clone());
        }
    }
    out.sort_by_key(|j| j.id);
    out
}
```

(No functional change — `Stopped` is already non-Running — but make sure `remove_notified` does NOT drop Stopped jobs. Continue.)

- [ ] **Step 11: Update `remove_notified` to keep Stopped jobs**

Replace `src/jobs.rs:136-139`:

```rust
pub fn remove_notified(&mut self) {
    self.jobs.retain(|j| {
        matches!(j.state, JobState::Running | JobState::Stopped(_)) || !j.notified
    });
}
```

- [ ] **Step 12: Update `decode_status` so Stopped raw statuses round-trip**

Replace `src/jobs.rs:209-220`:

```rust
fn decode_status(raw: libc::c_int) -> JobState {
    if libc::WIFEXITED(raw) {
        JobState::Done(libc::WEXITSTATUS(raw))
    } else if libc::WIFSIGNALED(raw) {
        JobState::Signaled(libc::WTERMSIG(raw))
    } else if libc::WIFSTOPPED(raw) {
        JobState::Stopped(libc::WSTOPSIG(raw))
    } else {
        JobState::Running
    }
}
```

- [ ] **Step 13: Run all jobs.rs tests — should pass**

Run: `cargo test --lib jobs::`
Expected: all green, including the two new tests and any pre-existing ones.

- [ ] **Step 14: Run the full suite to make sure column-width change didn't break anything**

Run: `cargo test`
Expected: 186 + 5 = 191 passed (or thereabouts — depends on whether any pre-existing test asserted the old `:<20` width verbatim).

If any test fails because of the column width change, update its expected string to width 24 and explain in the commit message.

- [ ] **Step 15: Commit**

```bash
git add src/jobs.rs
git commit -m "feat: add JobState::Stopped and reformat notifications"
```

---

## Task 2: `JobTable` lookups — `current_id`, `current_stopped_id`, `has_pending`

**Files:**
- Modify: `src/jobs.rs:91-93` (`has_running` → rename + extend), `src/jobs.rs:143-149` (add new helpers near `current_and_previous`)
- Modify: `src/builtins.rs` (find every `has_running` call site)
- Test: `src/jobs.rs`

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests`:

```rust
#[test]
fn current_id_returns_most_recent_running_or_stopped() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());      // id 1
    let _ = t.add(200, vec![200], "b".to_string());      // id 2 — more recent
    assert_eq!(t.current_id(), Some(2));
}

#[test]
fn current_id_includes_stopped_jobs() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());
    let _ = t.add(200, vec![200], "b".to_string());
    t.jobs_mut()[1].state = JobState::Stopped(libc::SIGTSTP);
    assert_eq!(t.current_id(), Some(2));
}

#[test]
fn current_id_returns_none_when_only_done_jobs() {
    let mut t = JobTable::new();
    let id = t.add(100, vec![100], "a".to_string());
    t.jobs_mut()[0].state = JobState::Done(0);
    assert_eq!(t.current_id(), None);
    let _ = id;
}

#[test]
fn current_stopped_id_skips_running_jobs() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());      // Running, id 1
    let _ = t.add(200, vec![200], "b".to_string());      // Running, id 2 (more recent)
    t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
    // Most-recent is id 2 (Running); current_stopped should skip it and return id 1.
    assert_eq!(t.current_stopped_id(), Some(1));
}

#[test]
fn has_pending_true_when_any_stopped() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());
    t.jobs_mut()[0].state = JobState::Stopped(libc::SIGTSTP);
    assert!(t.has_pending());
}

#[test]
fn has_pending_false_when_all_done() {
    let mut t = JobTable::new();
    let _ = t.add(100, vec![100], "a".to_string());
    t.jobs_mut()[0].state = JobState::Done(0);
    assert!(!t.has_pending());
}
```

- [ ] **Step 2: Run them — confirm failure**

Run: `cargo test --lib jobs::tests::current_id jobs::tests::current_stopped_id jobs::tests::has_pending`
Expected: FAIL — methods do not exist.

- [ ] **Step 3: Add the new helpers**

Insert after `src/jobs.rs:149` (just below `current_and_previous`):

```rust
/// Most-recent Running or Stopped job id (the `+` job for fg/bg/jobs).
pub fn current_id(&self) -> Option<u32> {
    let mut by_age: Vec<&Job> = self
        .jobs
        .iter()
        .filter(|j| matches!(j.state, JobState::Running | JobState::Stopped(_)))
        .collect();
    by_age.sort_by_key(|j| std::cmp::Reverse(j.created_at));
    by_age.first().map(|j| j.id)
}

/// Most-recent Stopped job id, ignoring Running jobs. Used by `bg`.
pub fn current_stopped_id(&self) -> Option<u32> {
    let mut by_age: Vec<&Job> = self
        .jobs
        .iter()
        .filter(|j| matches!(j.state, JobState::Stopped(_)))
        .collect();
    by_age.sort_by_key(|j| std::cmp::Reverse(j.created_at));
    by_age.first().map(|j| j.id)
}

/// True if any job is Running or Stopped (i.e., `wait` should block).
pub fn has_pending(&self) -> bool {
    self.jobs
        .iter()
        .any(|j| matches!(j.state, JobState::Running | JobState::Stopped(_)))
}
```

- [ ] **Step 4: Delete the old `has_running`**

Remove `src/jobs.rs:91-93`.

- [ ] **Step 5: Update the `builtin_wait` call site to use `has_pending`**

In `src/builtins.rs`, find `shell.jobs.has_running()` (there is exactly one call, inside `builtin_wait`) and replace with `shell.jobs.has_pending()`.

- [ ] **Step 6: Build to find any remaining call sites**

Run: `cargo build 2>&1 | grep -E "has_running|error\["`
Expected: empty (no errors, no remaining references).

If any other files referenced `has_running`, update those too.

- [ ] **Step 7: Run the tests**

Run: `cargo test --lib jobs::`
Expected: all green, 6 new tests added.

- [ ] **Step 8: Commit**

```bash
git add src/jobs.rs src/builtins.rs
git commit -m "feat: add current_id/current_stopped_id/has_pending to JobTable"
```

---

## Task 3: `Shell.shell_pgid` + ignore SIGTSTP/SIGTTIN/SIGTTOU

**Files:**
- Modify: `src/shell_state.rs:18-40` (add `shell_pgid: i32`)
- Modify: `src/shell.rs:17-30` (`run()` — install signal disposition), add new `install_job_control_signals` function
- Test: `src/shell_state.rs` (verify `shell_pgid` matches `getpgrp()`)

- [ ] **Step 1: Write the failing test**

Add inside `src/shell_state.rs`'s `mod tests`:

```rust
#[test]
fn new_captures_shell_pgid_from_getpgrp() {
    let s = Shell::new();
    let expected = unsafe { libc::getpgrp() };
    assert_eq!(s.shell_pgid, expected);
    assert!(s.shell_pgid > 0, "pgrp should be positive");
}
```

- [ ] **Step 2: Run it — confirm failure**

Run: `cargo test --lib shell_state::tests::new_captures_shell_pgid_from_getpgrp`
Expected: FAIL — `shell_pgid` field does not exist.

- [ ] **Step 3: Add the field and initialize it**

In `src/shell_state.rs`, find the `pub struct Shell { ... }` block (starts around line 18) and add `pub shell_pgid: i32,` to its fields. Then in `Shell::new()` (around line 27), set `shell_pgid: unsafe { libc::getpgrp() }`.

- [ ] **Step 4: Run the test — should pass**

Run: `cargo test --lib shell_state::tests::new_captures_shell_pgid_from_getpgrp`
Expected: PASS.

- [ ] **Step 5: Add the signal-disposition installer**

In `src/shell.rs`, just after `install_sigchld_handler` (around line 71), add:

```rust
/// Ignore SIGTSTP/SIGTTIN/SIGTTOU at the shell level so that:
///   - Ctrl-Z at the prompt does not suspend shuck itself.
///   - `tcsetpgrp` from a non-foreground pgrp does not trigger SIGTTOU on us.
///   - Defensive: shuck never reads `/dev/tty` directly today, but match bash.
fn install_job_control_signals() {
    unsafe {
        libc::signal(libc::SIGTSTP, libc::SIG_IGN);
        libc::signal(libc::SIGTTIN, libc::SIG_IGN);
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
    }
}
```

- [ ] **Step 6: Call it from `run`**

In `src/shell.rs::run` (around line 18), after `install_sigint_handler();` add a new line:

```rust
install_job_control_signals();
```

- [ ] **Step 7: Build to confirm everything compiles**

Run: `cargo build`
Expected: clean build, no warnings.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add src/shell_state.rs src/shell.rs
git commit -m "feat: capture shell_pgid and ignore SIGTSTP/SIGTTIN/SIGTTOU"
```

---

## Task 4: SIGCHLD reaper uses `WUNTRACED`; `reap` handles WIFSTOPPED

**Files:**
- Modify: `src/jobs.rs:99-119` (`reap` — handle stopped status), `src/jobs.rs:164-177` (`reap_completed` — use `WUNTRACED`)
- Test: `src/jobs.rs`

- [ ] **Step 1: Write the failing test**

Add inside `mod tests`:

```rust
#[test]
fn reap_with_stopped_status_transitions_job_to_stopped_state() {
    let mut t = JobTable::new();
    let _ = t.add(4242, vec![4242], "sleep 100".to_string());
    // Construct a raw waitpid status indicating WIFSTOPPED with SIGTSTP.
    // POSIX: low byte = 0x7f, second byte = stop signal.
    let raw_status: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
    t.reap(4242, raw_status);
    let j = &t.jobs_mut()[0];
    assert!(
        matches!(j.state, JobState::Stopped(s) if s == libc::SIGTSTP),
        "got state {:?}",
        j.state
    );
    assert!(!j.reaped[0], "stopped is not reaped — child still exists");
    assert!(!j.notified, "stopped jobs must be visible to the next notification pass");
}

#[test]
fn reap_with_exit_after_stop_finally_transitions_to_done() {
    let mut t = JobTable::new();
    let _ = t.add(4242, vec![4242], "sleep 100".to_string());
    // First a stop, then a clean exit 0.
    let stopped: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
    let exited: libc::c_int = 0; // WIFEXITED true, WEXITSTATUS 0
    t.reap(4242, stopped);
    assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(_)));
    t.reap(4242, exited);
    assert!(matches!(t.jobs_mut()[0].state, JobState::Done(0)));
}
```

- [ ] **Step 2: Run them — confirm failure**

Run: `cargo test --lib jobs::tests::reap_with_stopped`
Expected: FAIL — current `reap` only transitions when all pids are reaped.

- [ ] **Step 3: Update `reap` to handle WIFSTOPPED before marking reaped**

Replace `src/jobs.rs:99-119` (the entire `reap` function):

```rust
/// Marks `pid` as reaped with the given raw waitpid status. If the status
/// is a *stop* (WIFSTOPPED), transitions the job to Stopped without
/// marking the pid reaped (the process still exists). If the status is a
/// terminal exit/signal and this is the last stage, records the status;
/// when all pids of the job are reaped, transitions its overall state.
/// No-op if `pid` isn't owned by any job in the table.
pub fn reap(&mut self, pid: i32, raw_status: i32) {
    for job in self.jobs.iter_mut() {
        if let Some(idx) = job.pids.iter().position(|&p| p == pid) {
            if libc::WIFSTOPPED(raw_status) {
                job.state = JobState::Stopped(libc::WSTOPSIG(raw_status));
                job.notified = false;
                return;
            }
            if job.reaped[idx] {
                return;
            }
            job.reaped[idx] = true;
            if idx == job.pids.len() - 1 {
                job.last_status = Some(raw_status);
            }
            if job.reaped.iter().all(|&b| b) {
                let raw = job.last_status.unwrap_or(0);
                job.state = decode_status(raw);
            }
            return;
        }
    }
}
```

- [ ] **Step 4: Run the two new tests — should pass**

Run: `cargo test --lib jobs::tests::reap_with_stopped jobs::tests::reap_with_exit_after_stop`
Expected: PASS.

- [ ] **Step 5: Switch the reaper to use WUNTRACED**

Replace `src/jobs.rs:168-176` (inside `reap_completed`):

```rust
loop {
    let mut raw_status: libc::c_int = 0;
    let pid = unsafe {
        libc::waitpid(-1, &mut raw_status, libc::WNOHANG | libc::WUNTRACED)
    };
    if pid <= 0 {
        break;
    }
    shell.jobs.reap(pid as i32, raw_status);
}
```

- [ ] **Step 6: Run the whole suite**

Run: `cargo test`
Expected: all green, 2 new tests passing.

- [ ] **Step 7: Commit**

```bash
git add src/jobs.rs
git commit -m "feat: SIGCHLD reaper handles WIFSTOPPED via WUNTRACED"
```

---

## Task 5: Foreground pipeline — own pgrp + tcsetpgrp + WUNTRACED wait

**Files:**
- Modify: `src/executor.rs:446-503` (`run_subprocess`)
- Modify: `src/executor.rs:518-710` (`run_multi_stage` — wait loop only; spawn loop is touched too)
- Modify: `src/executor.rs:416-444` (`run_exec_single` — pass `&mut Shell` down)
- Modify: `src/executor.rs:405-414` (`run_single`)
- Test: `src/executor.rs` (verify ENOTTY swallowed; verify single-subprocess path returns 128+sig on simulated stop)

**Approach:** the new pgrp + tcsetpgrp + WUNTRACED behavior is **only** applied when the sink is `StdoutSink::Terminal`. Command-substitution callers (`StdoutSink::Capture`) keep the existing `child.wait()` semantics — substituting children shouldn't be Ctrl-Z'able from a user keystroke.

- [ ] **Step 1: Add helper functions in `src/executor.rs`**

At the bottom of `src/executor.rs`, just before `#[cfg(test)] mod tests`, add:

```rust
/// Best-effort: give the controlling terminal to `pgid`. Swallows ENOTTY
/// (non-tty environments like cargo test) and EPERM (race: pgrp already
/// exited). Other errors are silently ignored too — terminal handoff is
/// purely cosmetic from the shell's correctness perspective.
fn give_terminal_to(pgid: i32) {
    unsafe {
        let _ = libc::tcsetpgrp(libc::STDIN_FILENO, pgid);
    }
}

/// Block-wait for a single child pid with WUNTRACED. Returns:
///   `Ok((raw_status, stopped))` where `stopped` is true if WIFSTOPPED.
///   `Err(())` on waitpid failure (treat as terminated, status 1).
fn wait_with_untraced(pid: i32) -> Result<(libc::c_int, bool), ()> {
    let mut status: libc::c_int = 0;
    let r = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
    if r < 0 {
        return Err(());
    }
    Ok((status, libc::WIFSTOPPED(status)))
}
```

- [ ] **Step 2: Write the failing test**

Add to `src/executor.rs::tests`:

```rust
#[test]
fn give_terminal_to_silently_succeeds_on_non_tty() {
    // In `cargo test`, fd 0 is typically not a tty. The helper must not
    // panic or print anything.
    give_terminal_to(1);  // bogus pgid; tcsetpgrp will fail; we don't care.
    // If we got here, the helper swallowed errors as designed.
}
```

- [ ] **Step 3: Run it — should pass already (helper exists from step 1)**

Run: `cargo test --lib executor::tests::give_terminal_to_silently_succeeds_on_non_tty`
Expected: PASS.

- [ ] **Step 4: Update `run_subprocess` signature to take `&mut Shell`**

Change `src/executor.rs:446-451` from:

```rust
fn run_subprocess(
    cmd: &ResolvedCommand,
    files: StageFiles,
    shell: &Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
```

to:

```rust
fn run_subprocess(
    cmd: &ResolvedCommand,
    files: StageFiles,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
```

Compilation will likely break at the caller in `run_exec_single`. Update `run_exec_single` to pass `&mut Shell` instead of `&Shell` — the function already takes `&mut Shell`, so just thread it.

Run: `cargo build`
Expected: clean build.

- [ ] **Step 5: Add the foreground job-control path in `run_subprocess`**

Replace the entire body of `run_subprocess` (`src/executor.rs:446-503`) with:

```rust
fn run_subprocess(
    cmd: &ResolvedCommand,
    files: StageFiles,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let interactive = matches!(sink, StdoutSink::Terminal);

    let mut process = ProcessCommand::new(&cmd.program);
    process.args(&cmd.args);
    process.env_clear();
    process.envs(shell.exported_env());
    if let Some(file) = files.stdin {
        process.stdin(Stdio::from(file));
    }
    let want_capture = matches!(sink, StdoutSink::Capture(_));
    if let Some(file) = files.stdout {
        process.stdout(Stdio::from(file));
    } else if want_capture {
        process.stdout(Stdio::piped());
    }
    if let Some(file) = files.stderr {
        process.stderr(Stdio::from(file));
    }

    if interactive {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }

    let mut child = match process.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            eprintln!("shuck: command not found: {}", cmd.program);
            return ExecOutcome::Continue(127);
        }
        Err(e) => {
            eprintln!("shuck: {}: {e}", cmd.program);
            return ExecOutcome::Continue(1);
        }
    };

    let pid = child.id() as i32;

    if interactive {
        // Race fix: parent calls setpgid too (idempotent with child's).
        unsafe { libc::setpgid(pid, pid); }
        give_terminal_to(pid);
    }

    // Capture child stdout for substitution before waiting (so the pipe
    // drains and the child can exit).
    let mut copy_err: Option<io::Error> = None;
    if let StdoutSink::Capture(buf) = sink {
        if let Some(mut child_stdout) = child.stdout.take() {
            if let Err(e) = io::copy(&mut child_stdout, *buf) {
                copy_err = Some(e);
            }
        }
    }

    let result = if interactive {
        let outcome = match wait_with_untraced(pid) {
            Err(()) => {
                eprintln!("shuck: {}: waitpid failed", cmd.program);
                ExecOutcome::Continue(1)
            }
            Ok((status, true)) => {
                // Stopped — register the job, print notification, return 128+sig.
                let sig = libc::WSTOPSIG(status);
                let display = cmd.program.clone();
                let id = shell.jobs.add(pid, vec![pid], display.clone());
                // Mark as Stopped so jobs / fg / bg see it correctly.
                if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
                    job.state = JobState::Stopped(sig);
                    job.notified = true;  // we're about to print it ourselves
                }
                eprintln!("\n[{id}]+ Stopped              {display}");
                // Forget the Child so Drop doesn't try to wait again.
                std::mem::forget(child);
                ExecOutcome::Continue(128 + sig)
            }
            Ok((status, false)) => {
                let exit = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else if libc::WIFSIGNALED(status) {
                    128 + libc::WTERMSIG(status)
                } else {
                    1
                };
                // Forget the Child so Drop doesn't wait again.
                std::mem::forget(child);
                ExecOutcome::Continue(exit)
            }
        };
        give_terminal_to(shell.shell_pgid);
        outcome
    } else {
        match child.wait() {
            Ok(status) => {
                if let Some(e) = copy_err {
                    eprintln!("shuck: {}: {e}", cmd.program);
                    ExecOutcome::Continue(1)
                } else {
                    ExecOutcome::Continue(status_code(&status))
                }
            }
            Err(e) => {
                eprintln!("shuck: {}: {e}", cmd.program);
                ExecOutcome::Continue(1)
            }
        }
    };

    result
}
```

Note: `shell.jobs.jobs_mut()` was added in Task 1 Step 6 as a non-test accessor — it's already available for this executor code to call.

- [ ] **Step 6: Add a matching foreground path to `run_multi_stage`**

This is the most surgical change. Inside `run_multi_stage` (`src/executor.rs:518-710`), make these changes:

a. After the resolve/open-files step, capture `let interactive = matches!(sink, StdoutSink::Terminal);` and `let mut first_pid: Option<i32> = None;`.

b. In the spawn loop, where `let mut process = ProcessCommand::new(&cmd.program);` is built (around line 617), after `process.envs(shell.exported_env());`, add:

```rust
if interactive {
    use std::os::unix::process::CommandExt;
    let pgid_target = first_pid.unwrap_or(0);
    process.process_group(pgid_target);
}
```

c. Immediately after `let mut child = match process.spawn() { ... };` succeeds (around line 651-669), and after the `pending_input` block, add:

```rust
let pid = child.id() as i32;
if interactive && first_pid.is_none() {
    first_pid = Some(pid);
    unsafe { libc::setpgid(pid, pid); }
}
```

d. After the spawn loop completes, if `interactive && first_pid.is_some()`, call `give_terminal_to(first_pid.unwrap())`.

e. Replace the wait loop (`src/executor.rs:694-708`):

```rust
let mut last_status = 0;
let mut stopped_sig: Option<i32> = None;
let mut stage_pids: Vec<i32> = Vec::new();
for stage in stages {
    match stage {
        Stage::Done(code) => last_status = code,
        Stage::Process(child) => {
            let pid = child.id() as i32;
            stage_pids.push(pid);
            if interactive {
                match wait_with_untraced(pid) {
                    Ok((status, true)) => {
                        stopped_sig = Some(libc::WSTOPSIG(status));
                        std::mem::forget(child);
                        break;
                    }
                    Ok((status, false)) => {
                        last_status = if libc::WIFEXITED(status) {
                            libc::WEXITSTATUS(status)
                        } else if libc::WIFSIGNALED(status) {
                            128 + libc::WTERMSIG(status)
                        } else {
                            1
                        };
                        std::mem::forget(child);
                    }
                    Err(()) => {
                        last_status = 1;
                        std::mem::forget(child);
                    }
                }
            } else {
                last_status = match child.wait() {
                    Ok(status) => status_code(&status),
                    Err(e) => {
                        eprintln!("shuck: {e}");
                        1
                    }
                };
            }
        }
    }
}

if interactive {
    if let Some(pgid) = first_pid {
        if let Some(sig) = stopped_sig {
            // Register the job in Stopped state. Use the first stage's
            // program name as the display (multi-stage display would
            // require threading the original source through; punt).
            let display = format!("(pipeline pid {pgid})");
            let id = shell.jobs.add(pgid, stage_pids.clone(), display.clone());
            if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
                job.state = JobState::Stopped(sig);
                job.notified = true;
            }
            eprintln!("\n[{id}]+ Stopped              {display}");
            give_terminal_to(shell.shell_pgid);
            return ExecOutcome::Continue(128 + sig);
        }
        give_terminal_to(shell.shell_pgid);
    }
}

ExecOutcome::Continue(last_status)
```

- [ ] **Step 7: Build, expect clean**

Run: `cargo build`
Expected: clean build.

- [ ] **Step 8: Run the full test suite**

Run: `cargo test`
Expected: all green. Pre-existing tests that spawn real subprocesses (`echo`, `cat`, etc.) in capture mode are unaffected because they take the `interactive == false` branch. Foreground subprocess tests in `cargo test` run with non-tty stdin; `give_terminal_to` swallows ENOTTY; `wait_with_untraced` blocks on the child the same way `child.wait()` did.

- [ ] **Step 9: Commit**

```bash
git add src/executor.rs src/jobs.rs
git commit -m "feat: foreground pipelines spawn in own pgrp with tcsetpgrp + WUNTRACED"
```

---

## Task 6: `builtin_fg` and `builtin_bg`

**Files:**
- Modify: `src/builtins.rs:16-25` (`is_builtin`), `src/builtins.rs::run_builtin` dispatch table
- Add: `fn builtin_fg(...)`, `fn builtin_bg(...)` in `src/builtins.rs`
- Test: `src/builtins.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/builtins.rs::tests` (or create the module if none exists):

```rust
#[cfg(test)]
mod fg_bg_tests {
    use super::*;
    use crate::jobs::JobState;
    use crate::shell_state::Shell;

    #[test]
    fn fg_with_no_jobs_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn bg_with_no_jobs_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn fg_with_args_rejected_with_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("fg", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn bg_with_args_rejected_with_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &["%1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn bg_on_running_job_returns_no_current_job() {
        // bg looks only at Stopped jobs. If the only job is Running, bg
        // should report "no current job".
        let mut shell = Shell::new();
        shell.jobs.add(4242, vec![4242], "sleep 100".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("bg", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn is_builtin_recognizes_fg_and_bg() {
        assert!(is_builtin("fg"));
        assert!(is_builtin("bg"));
    }
}
```

- [ ] **Step 2: Run them — confirm failure**

Run: `cargo test --lib builtins::fg_bg_tests`
Expected: FAIL — `fg` and `bg` not recognized; dispatch returns "command not found" or panics.

- [ ] **Step 3: Update `is_builtin`**

In `src/builtins.rs:16-25`, add `"fg"` and `"bg"` to the matcher. The current implementation is a `matches!` macro; add the two strings.

- [ ] **Step 4: Add dispatch entries in `run_builtin`**

Locate the `match name` block inside `run_builtin` and add two new arms:

```rust
"fg" => builtin_fg(args, shell),
"bg" => builtin_bg(args, out, shell),
```

- [ ] **Step 5: Implement `builtin_fg`**

Add to `src/builtins.rs`:

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
    let (pgid, pids, command) = {
        let job = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id).unwrap();
        // Mark Running so it isn't re-notified mid-resume.
        job.state = crate::jobs::JobState::Running;
        job.notified = true;
        (job.pgid, job.pids.clone(), job.command.clone())
    };

    eprintln!("{command}");

    unsafe {
        libc::tcsetpgrp(libc::STDIN_FILENO, pgid);
        libc::killpg(pgid, libc::SIGCONT);
    }

    let mut last_status = 0;
    let mut stopped_sig: Option<i32> = None;
    for &pid in &pids {
        let mut status: libc::c_int = 0;
        let r = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if r < 0 {
            last_status = 1;
            continue;
        }
        if libc::WIFSTOPPED(status) {
            stopped_sig = Some(libc::WSTOPSIG(status));
            break;
        }
        last_status = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else if libc::WIFSIGNALED(status) {
            128 + libc::WTERMSIG(status)
        } else {
            1
        };
    }

    unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, shell.shell_pgid); }

    if let Some(sig) = stopped_sig {
        if let Some(job) = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id) {
            job.state = crate::jobs::JobState::Stopped(sig);
            job.notified = true;
        }
        eprintln!("\n[{id}]+ Stopped              {command}");
        return ExecOutcome::Continue(128 + sig);
    }

    // Job exited cleanly — drop it from the table so the next prompt
    // doesn't reprint it.
    shell.jobs.jobs_mut().retain(|j| j.id != id);
    ExecOutcome::Continue(last_status)
}
```

- [ ] **Step 6: Implement `builtin_bg`**

Add to `src/builtins.rs`:

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
    let (pgid, command) = {
        let job = shell.jobs.jobs_mut().iter_mut().find(|j| j.id == id).unwrap();
        job.state = crate::jobs::JobState::Running;
        job.notified = true;
        (job.pgid, job.command.clone())
    };

    unsafe { libc::killpg(pgid, libc::SIGCONT); }

    eprintln!("[{id}]+ {command} &");
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 7: Build and run the tests**

Run: `cargo test --lib builtins::fg_bg_tests`
Expected: PASS, 6 tests.

- [ ] **Step 8: Run the full suite**

Run: `cargo test`
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add src/builtins.rs
git commit -m "feat: add fg and bg builtins (no-arg form)"
```

---

## Task 7: Smoke test against a real terminal

**Files:** none (manual verification only)

Job control's interesting paths only fire under a real controlling terminal. `cargo test` runs without one, so this iteration's automated tests cover state transitions but not signal delivery or `tcsetpgrp`. Verify the following manually.

- [ ] **Step 1: Build the release binary**

Run: `cargo build --release`
Expected: clean build at `target/release/shuck`.

- [ ] **Step 2: Run shuck in a real terminal and walk the checklist**

Run: `target/release/shuck`

Then perform each of these and confirm the output matches:

1. **Ctrl-Z a long-running command**
   - Type `sleep 100` and press Enter.
   - Press Ctrl-Z.
   - Expected: `[1]+ Stopped              sleep 100` then a fresh `shuck> ` prompt.

2. **`jobs` shows the stopped job**
   - Type `jobs` and press Enter.
   - Expected: `[1]+ Stopped              sleep 100`.

3. **`fg` resumes in foreground**
   - Type `fg` and press Enter.
   - Expected: echoes `sleep 100` and blocks.
   - Press Ctrl-Z again.
   - Expected: another `[1]+ Stopped` line, fresh prompt.

4. **`bg` resumes in background**
   - Type `bg` and press Enter.
   - Expected: `[1]+ sleep 100 &`, fresh prompt.

5. **`jobs` shows Running**
   - Type `jobs`.
   - Expected: `[1]+ Running              sleep 100`.

6. **Wait for it to finish**
   - Wait ~100s (or `kill %1` from another terminal — we don't have `kill %1` yet, so use `kill <pid>`).
   - At the next prompt, expect `[1]+ Done                 sleep 100 &`.

7. **vim takes the terminal cleanly**
   - Type `vim /tmp/shuck-test` and press Enter.
   - Expected: vim opens normally.
   - Press Ctrl-Z.
   - Expected: returns to shuck with `[1]+ Stopped              vim`.
   - Type `fg`.
   - Expected: vim resumes.
   - In vim, type `:q!` and press Enter.
   - Expected: back at shuck prompt.

8. **Background command that needs the terminal stops with SIGTTIN**
   - Type `cat &` and press Enter.
   - Expected: `[1] <pid>` then shortly `[1]+ Stopped (tty input)  cat`.

9. **`wait` blocks on a stopped job**
   - Make sure you have a stopped job (Ctrl-Z `sleep 100` if you don't).
   - Type `wait`.
   - Expected: blocks indefinitely. Press Ctrl-C to interrupt.

10. **Ctrl-Z at empty prompt does nothing**
    - At a fresh prompt, press Ctrl-Z.
    - Expected: shuck does NOT suspend. (Some terminals echo `^Z` to the screen; that's fine — what matters is that the shell stays interactive.)

11. **Exit**
    - Type `exit`.
    - Expected: shuck exits cleanly.

- [ ] **Step 3: Capture and commit smoke-test results**

After confirming all 11 items, do NOT commit anything — this is verification only. Just note in the PR description (or follow-up message) which items passed.

If any item fails, file a fix task and address it before the next task. Do not proceed to merge.

---

## Wrap-up

- [ ] **Final code review**

After Task 7, dispatch the `feature-dev:code-reviewer` agent across the full branch diff against `main`, in the same shape as v6:

```
git diff main...HEAD
```

Focus areas:
- Signal-handling correctness (SIG_IGN install ordering vs. SIGCHLD handler).
- `tcsetpgrp` race windows (post-spawn before parent setpgid; child exited before parent gives terminal).
- `std::mem::forget(child)` correctness — does our manual `waitpid` actually reap, or do we leak?
- Capture-mode regression: does the `interactive == false` branch in `run_subprocess` behave exactly like pre-v7?
- Notification format: `[N]+ Stopped <cmd>` vs `[N]+ Done <cmd> &` — does the column-width bump break any pre-existing test?

- [ ] **Apply review fixes; merge to main**

Same flow as v6: `--no-ff` merge, delete the branch, push.

---

## Self-review notes

**Spec coverage:**
- §1 (Job state and notifications) → Task 1.
- §2 (Foreground execution path) → Task 5.
- §3 (`fg`/`bg`) → Task 6.
- §4 (Signal disposition, `wait`, `jobs`) → Task 3 (signals), Task 2 (`has_pending`), Task 1 (render).
- §5 (Edge cases) → covered via specific tests in Task 5 (ENOTTY) and Task 6 (no-current-job, args).
- §6 (Testing) → Task 1, 2, 4, 5, 6 unit tests + Task 7 manual checklist.
- §7 (File summary) → matches Task 1–6 file lists.
- §8 (Out of scope) → no implementation; documented as deferral.

**Placeholder scan:** No TBD/TODO/"similar to" steps. Every code step has the complete code.

**Type consistency:** `JobTable::jobs_mut()` is added in Task 1 Step 6 and reused by Tasks 2, 4, 5, 6. `notification_line(&Job, char) -> String` (Task 1 Step 6) is the single format-source. `wait_with_untraced(pid) -> Result<(libc::c_int, bool), ()>` (Task 5 Step 1) is reused inline by `builtin_fg` (Task 6) — implementer should consider extracting if duplication grows.

**Branching:** Branch off `main` (Step 0a). All tasks commit on the feature branch; the wrap-up merges via `--no-ff` matching v6's pattern.
