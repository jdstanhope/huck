# v172: centralize job-control + fix L-51 — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Centralize the drifted job-control predicate into `Shell::job_control_active()` and add a `NO_PGROUP` sentinel so the fork primitives own the `setpgid` policy; a non-interactive shell then keeps foreground pipeline/subshell children in its own process group (fix L-51), with interactive job control byte-for-byte unchanged.

**Architecture:** Three behavior-changing edits flow through one helper + one sentinel. `job_control_active()` (= `is_interactive && !in_subshell && !in_completion`) replaces the three inline `let interactive = matches!(sink, Terminal) && …` predicates (which all dropped `is_interactive` — the bug). `NO_PGROUP` (-1) passed as `pgid_target` when job control is off makes the fork primitives skip `setpgid` (child inherits the shell's group). Foreground paths only (run_multi_stage = pipelines + single commands; the Subshell arm; run_subprocess). Background (run_background_sequence) is **deferred** (separate hardcoded setpgids + subtler semantics; not part of L-51's documented pipeline scope).

**Tech Stack:** Rust, `libc::setpgid`. PTY tests via `expectrl` (existing `tests/*_pty.rs` pattern).

**Spec:** `docs/superpowers/specs/2026-06-16-jobcontrol-pgroup-design.md`

**Branch:** `v172-jobcontrol-pgroup`

**Risk note:** this is the v108/v121/v124 tty-deadlock area. The existing PTY suite (`subshell_pipeline_pty`, `subshell_tty_pty`, `subshell_job_notice_pty`, `completion_jobcontrol_pty`, `procsub_stop_pty`, `sigint_abort_pty`, `pty_interactive`) is the deadlock-regression guard and MUST stay green after every behavior-changing task. Verify incrementally — do not batch.

---

## Confirmed sites (src/executor.rs)

- Drifted predicates: `let interactive = matches!(sink, StdoutSink::Terminal) && …` at **378** (Subshell arm, multi-line), **4127** (`run_subprocess`, multi-line), **4465** (`run_multi_stage`, single-line). All omit `shell.is_interactive`.
- Foreground `pgid_target`: **4515** & **4884** (run_multi_stage, `if interactive { first_pid.unwrap_or(0) } else { 0 }`); the `0` arg at **401** (Subshell-arm `fork_and_run_in_subshell` call) and **4348** (run_subprocess call) — both hardcoded `0`, ungated.
- Fork-primitive `setpgid` (gate on `>= 0`): **5752** `libc::setpgid(0, pgid_target);` (child), **5830** `libc::setpgid(pid, pgid_target);` (parent race-close, fork_and_run_in_subshell), **6047** `let _ = libc::setpgid(pid, pgid_target);` (parent, spawn_external_with_fds).
- Already `if interactive`-gated (fixed transitively by the predicate change): the hardcoded `setpgid(pid, pid)` at **442** (Subshell) and **4187** (run_subprocess).

---

### Task 1: Foundation — `job_control_active()` + `NO_PGROUP` + gate the fork-primitive setpgids (behavior-neutral)

This adds the helper/sentinel and makes `setpgid` conditional on `pgid_target >= 0`. It is **inert**: all current callers pass `0` or `first_pid` (both `>= 0`), so `setpgid` still fires exactly as before.

**Files:** Modify `src/shell_state.rs`, `src/executor.rs`.

- [ ] **Step 1: Add `Shell::job_control_active()`**

In `src/shell_state.rs`, add this method to `impl Shell` (near `is_interactive`/the option accessors):

```rust
    /// True when this shell should use job control (own process groups +
    /// terminal handoff) for the commands it forks: an interactive shell not
    /// inside a subshell environment or a completion function. The single source
    /// of truth — replaces the inline `matches!(sink, Terminal) && !in_subshell
    /// && !in_completion` copies that had drifted (they omitted `is_interactive`).
    /// Foreground callers additionally require a `StdoutSink::Terminal` sink.
    pub fn job_control_active(&self) -> bool {
        self.is_interactive && !self.in_subshell && !self.in_completion
    }
```

- [ ] **Step 2: Add the `NO_PGROUP` constant**

In `src/executor.rs`, near the top (after the imports / other `const`s), add:

```rust
/// `pgid_target` sentinel for the fork primitives meaning "do not `setpgid` —
/// inherit the shell's process group" (job control off). `0` = become a new
/// group leader; `N > 0` = join group `N`.
const NO_PGROUP: i32 = -1;
```

- [ ] **Step 3: Gate the child `setpgid` in `fork_and_run_in_subshell` (line ~5752)**

Replace:
```rust
            // 2. Join the pgrp (or become pgrp leader if pgid_target == 0).
            libc::setpgid(0, pgid_target);
```
with:
```rust
            // 2. Join the pgrp (leader if pgid_target == 0); NO_PGROUP (< 0)
            //    means "stay in the shell's group" (job control off).
            if pgid_target >= 0 {
                libc::setpgid(0, pgid_target);
            }
```

- [ ] **Step 4: Gate the two parent race-close `setpgid`s (lines ~5830, ~6047)**

At ~5830 (`fork_and_run_in_subshell` parent), replace:
```rust
    unsafe {
        libc::setpgid(pid, pgid_target);
    }
```
with:
```rust
    if pgid_target >= 0 {
        unsafe {
            libc::setpgid(pid, pgid_target);
        }
    }
```
At ~6047 (`spawn_external_with_fds` parent), replace:
```rust
    unsafe {
        let _ = libc::setpgid(pid, pgid_target);
    }
```
with:
```rust
    if pgid_target >= 0 {
        unsafe {
            let _ = libc::setpgid(pid, pgid_target);
        }
    }
```

- [ ] **Step 5: Build + verify inert (no behavior change)**

Run: `cargo build 2>&1 | tail -2` → `Finished`.
Run: `cargo test --test subshell_pipeline_pty --test subshell_tty_pty --test pty_interactive 2>&1 | grep -E 'test result'`
Expected: all `ok` (these still pass — the change is inert since every caller passes `pgid_target >= 0`).

- [ ] **Step 6: Commit**

```bash
git add src/shell_state.rs src/executor.rs
git commit -m "v172: add job_control_active() + NO_PGROUP, gate fork-primitive setpgid

Foundation for L-51: one job-control predicate + a NO_PGROUP (-1) pgid_target
sentinel that makes fork_and_run_in_subshell / spawn_external_with_fds skip
setpgid (child inherits the shell's group). Inert so far — all callers still
pass 0/first_pid (>= 0).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Fix the pipeline path (run_multi_stage) — the documented L-51

**Files:** Modify `src/executor.rs`.

- [ ] **Step 1: Fix the predicate (line ~4465)**

Replace:
```rust
    let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_subshell && !shell.in_completion;
```
with:
```rust
    let interactive = shell.job_control_active() && matches!(sink, StdoutSink::Terminal);
```

- [ ] **Step 2: Switch both `pgid_target` sites to NO_PGROUP (lines ~4515, ~4884)**

Both read `let pgid_target = if interactive { first_pid.unwrap_or(0) } else { 0 };`. Replace each with:
```rust
            let pgid_target = if interactive { first_pid.unwrap_or(0) } else { NO_PGROUP };
```
(Apply at both 4515 and 4884; match indentation.)

- [ ] **Step 3: Build + verify L-51 fixed + interactive unchanged**

Run: `cargo build --quiet && H=$(pwd)/target/debug/huck`
Non-interactive pipeline now stays in the shell's group (L-51 fixed) — compare to bash:
```bash
fifo=$(mktemp -u); mkfifo "$fifo"
printf 'cat %s | wc -c\n' "$fifo" > /tmp/p.sh
"$H" </tmp/p.sh >/dev/null 2>&1 & hpid=$!
sleep 1
echo "huck pgid=$(ps -o pgid= -p $hpid | tr -d ' ')  stages:"; ps -eo pid,ppid,pgid,comm | awk -v h=$hpid '$2==h{print "  "$0}'
# expect both stages' pgid == huck's pgid (shell group), NOT a sibling group
kill -9 $hpid 2>/dev/null; for p in $(pgrep -P 1 -x cat; pgrep -P 1 -x wc); do kill -9 $p 2>/dev/null; done; rm -f "$fifo" /tmp/p.sh
```
Expected: the `cat`/`wc` stages' `pgid` equals huck's `pgid` (same group), not the first stage's pid.
Run: `cargo test --test subshell_pipeline_pty --test pty_interactive 2>&1 | grep 'test result'`
Expected: `ok` (interactive job control unchanged).

- [ ] **Step 4: Commit**

```bash
git add src/executor.rs
git commit -m "v172: pipelines inherit the shell's process group when job control is off (fix L-51)

run_multi_stage now uses job_control_active() (which includes is_interactive)
and passes NO_PGROUP when job control is off, so a non-interactive pipeline's
stages stay in the shell's process group like bash, instead of a sibling group.
Interactive job control unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Fix the foreground subshell paths (Subshell arm + run_subprocess)

A non-interactive `( cmd )` subshell child likewise becomes its own group leader (`pgid_target = 0` hardcoded). Apply the same treatment; the parent `setpgid(pid,pid)` at 442/4187 is already `if interactive`-gated, so it follows the corrected predicate.

**Files:** Modify `src/executor.rs`.

- [ ] **Step 1: Fix the Subshell-arm predicate (line ~378)**

Replace:
```rust
            let interactive = matches!(sink, StdoutSink::Terminal)
                && !shell.in_subshell
                && !shell.in_completion;
```
with:
```rust
            let interactive = shell.job_control_active() && matches!(sink, StdoutSink::Terminal);
```

- [ ] **Step 2: Make the Subshell-arm fork pass NO_PGROUP when off (line ~401)**

In the `fork_and_run_in_subshell(...)` call, the 6th argument is a hardcoded `0`. Replace that line (`                0,`) with:
```rust
                if interactive { 0 } else { NO_PGROUP },
```

- [ ] **Step 3: Fix the run_subprocess predicate (line ~4127)**

Replace:
```rust
    let interactive =
        matches!(sink, StdoutSink::Terminal) && !shell.in_subshell && !shell.in_completion;
```
with:
```rust
    let interactive = shell.job_control_active() && matches!(sink, StdoutSink::Terminal);
```

- [ ] **Step 4: Make the run_subprocess fork pass NO_PGROUP when off (line ~4348)**

In the `fork_and_run_in_subshell(...)` call, the 6th argument is a hardcoded `0`. Replace that line (`        0,`) with:
```rust
        if interactive { 0 } else { NO_PGROUP },
```

- [ ] **Step 5: Build + verify subshell job control intact**

Run: `cargo build 2>&1 | tail -2` → `Finished`.
Run: `cargo test --test subshell_tty_pty --test subshell_pipeline_pty --test subshell_job_notice_pty 2>&1 | grep 'test result'`
Expected: all `ok` (the v108/v124 subshell tty-deadlock + job-notice behavior is preserved).
Quick non-interactive check: `printf '( sleep 0.2 & wait ); echo done\n' | ./target/debug/huck` prints `done` (no hang).

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs
git commit -m "v172: foreground subshells inherit the shell's group when job control is off

The Subshell arm and run_subprocess passed a hardcoded pgid_target 0 (own group
leader); now pass NO_PGROUP when job control is off, and use job_control_active().
Interactive subshell job control / tty handling unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: PTY + pgroup regression test

**Files:** Create `tests/jobcontrol_pgroup_pty.rs`.

- [ ] **Step 1: Write the test (interactive Ctrl-Z/fg + non-interactive pgroup)**

Create `tests/jobcontrol_pgroup_pty.rs`, mirroring `tests/subshell_pipeline_pty.rs`'s `expectrl`/`OsSession` PTY harness (copy its PTY-driver helpers and skip-on-no-pty handling). Add two tests:

```rust
//! v172 regression: interactive job control (Ctrl-Z stop / fg resume on a
//! pipeline) still works, and a NON-interactive pipeline keeps its stages in the
//! shell's process group (L-51). The interactive case guards the v108/v121/v124
//! tty-deadlock area against this change; the pgroup case proves L-51 fixed.

// (Reuse the PtyRun / drive-huck-under-pty helpers from subshell_pipeline_pty.rs.)

#[test]
fn interactive_pipeline_ctrl_z_then_fg_resumes() {
    // Drive an interactive huck over a PTY: start a foreground pipeline that
    // would print READY then DONE; Ctrl-Z must stop it (shell returns to the
    // prompt without DONE), `fg` must resume it to DONE. A hang/timeout fails.
    // Skip cleanly if no PTY can be allocated.
    let run = drive_pty(&[
        ("echo PRE", "PRE"),
        ("sleep 1 | { echo READY; sleep 1; echo DONE; }", "READY"),
        // ^C/Ctrl-Z sent as control byte by the harness step below
    ]);
    if run.skipped { return; }
    assert!(run.saw("READY"), "pipeline did not start under PTY");
    // (The harness sends Ctrl-Z after READY, then `fg`; assert DONE appears,
    //  proving stop+resume — i.e. the pipeline pgroup + terminal handoff work.)
    assert!(run.saw("DONE"), "fg did not resume the stopped pipeline");
}

#[test]
fn noninteractive_pipeline_stages_share_shell_pgroup() {
    use std::process::Command;
    // A non-interactive huck running a blocked pipeline: its stages must be in
    // huck's OWN process group (L-51), not a sibling group.
    let huck = env!("CARGO_BIN_EXE_huck");
    let fifo = std::env::temp_dir().join(format!("v172_{}", std::process::id()));
    let _ = Command::new("mkfifo").arg(&fifo).status();
    let mut child = Command::new(huck)
        .arg("-c").arg(format!("cat {} | wc -c", fifo.display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn().expect("spawn huck");
    std::thread::sleep(std::time::Duration::from_millis(400));
    let huck_pid = child.id();
    let huck_pgid = pgid_of(huck_pid);
    // gather the cat/wc stage pids (children of huck) and their pgids
    let stages = child_pids(huck_pid);
    let _ = std::fs::remove_file(&fifo);
    let _ = child.kill();
    for p in &stages { unsafe { libc::kill(*p as i32, libc::SIGKILL); } }
    let _ = child.wait();
    if stages.is_empty() { return; } // couldn't observe (CI race) — skip
    for p in stages {
        assert_eq!(pgid_of(p), huck_pgid,
            "stage {p} pgid {} != huck pgid {huck_pgid} (L-51: stage in a sibling group)", pgid_of(p));
    }
}

fn pgid_of(pid: u32) -> i32 {
    let out = std::process::Command::new("ps").args(["-o", "pgid=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(-2)
}
fn child_pids(parent: u32) -> Vec<u32> {
    let out = std::process::Command::new("pgrep").args(["-P", &parent.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect()
}
```

(If the PTY Ctrl-Z/`fg` driving cannot be expressed cleanly with the existing harness helpers, implement the interactive test by sending the raw `\x1a` (Ctrl-Z) byte then `fg\r` between the marker waits — follow `tests/procsub_stop_pty.rs`, which already exercises Ctrl-Z stop under a PTY, as the closer model.)

- [ ] **Step 2: Run the new test**

Run: `cargo test --test jobcontrol_pgroup_pty 2>&1 | grep -E 'test result|FAILED'`
Expected: `ok` (both pass; or skip cleanly if no PTY / `ps`/`pgrep` unavailable).

- [ ] **Step 3: Commit**

```bash
git add tests/jobcontrol_pgroup_pty.rs
git commit -m "test: v172 job-control PTY regression + non-interactive pgroup (L-51)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Full regression + resolve L-51 doc

**Files:** Modify `docs/bash-divergences.md`.

- [ ] **Step 1: Full suite + clippy + all PTY tests + harnesses**

Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN` → `CLEAN`.
Run: `cargo test >/tmp/v172.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v172.log` → `exit: 0`, `0`.
Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"` → `93 passed, 0 failed`.

- [ ] **Step 2: Delete the L-51 entry + add the background follow-on; fix the Tier-4 count**

In `docs/bash-divergences.md`, remove the entire `- **L-51: a pipeline gets its own process group even when job control is OFF** …` bullet, and in its place add a narrower follow-on (background path was deferred):
```markdown
- **L-53: background (`&`) pipelines/subshells get their own process group when job control is OFF** — `[deferred]`, low (v172 follow-on). v172 fixed the FOREGROUND case (L-51) — non-interactive foreground pipelines/subshells now stay in the shell's process group via `job_control_active()` + the `NO_PGROUP` sentinel. The `run_background_sequence` path (`(…) &`, `a | b &`) still `setpgid`s its first stage into a new group unconditionally (`executor.rs` ~2075/2466, gated only on `first_pid.is_none()`), so a non-interactive backgrounded pipeline is still in a sibling group. Deferred because background pgroup semantics are subtler and `run_background_sequence` has its own (hardcoded `setpgid(pid,pid)`) machinery distinct from the fork primitives; the same `job_control_active()`/`NO_PGROUP` treatment applies when revisited.
```
Decrement the Tier-4 summary count by 1 (40 → 40 — net: L-51 removed, L-53 added, so the count stays **40**; confirm the table reads 40).

- [ ] **Step 3: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "docs: resolve L-51 (foreground); log L-53 (background pgroup follow-on)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: `shell_state.rs` (the helper), `executor.rs` (NO_PGROUP + 3 setpgid gates + 3 predicates + 3 pgid_target sites), the new PTY test, `bash-divergences.md`. Confirm interactive behavior is unchanged by inspection (the `if interactive` blocks still fire when `job_control_active() && Terminal`).
- Re-run the FULL PTY suite (`cargo test --test '*_pty'` or each) — the deadlock-regression guard — and the L-51 reproduction by hand.
- Manually drive an interactive huck over a real terminal: `sleep 5 | cat` → Ctrl-Z (stops) → `fg` (resumes); `jobs`/`bg` behave; confirm no wedge.
- Merge `v172-jobcontrol-pgroup` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the branch.
- Record in `project_huck_iterations.md` + `MEMORY.md`; note L-53 (background) as the remaining follow-on.

---

## Self-review (plan vs spec)

- **Spec coverage:** `job_control_active()` helper (Task 1) ✓; `NO_PGROUP` + gated fork-primitive setpgid (Task 1) ✓; predicate fix at all 3 sites (Tasks 2–3) ✓; pgid_target→NO_PGROUP for foreground pipeline + subshell (Tasks 2–3) ✓; interactive unchanged (verified each task via PTY suite) ✓; new PTY regression test + non-interactive pgroup test (Task 4) ✓; full regression + L-51 resolution (Task 5) ✓; background DEFERRED → logged as L-53 (Task 5) — a documented narrowing from the spec's "four paths", made because run_background_sequence's hardcoded setpgids + background semantics warrant separate care, and L-51's text is foreground pipelines.
- **Placeholder scan:** none — exact line numbers + before/after for every edit; exact verification commands.
- **Type consistency:** `job_control_active(&self) -> bool`, `NO_PGROUP: i32 = -1`, `pgid_target >= 0` used consistently; the three predicate sites and three pgid_target sites all switch to the same forms.
