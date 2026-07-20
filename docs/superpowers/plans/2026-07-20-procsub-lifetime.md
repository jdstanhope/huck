# v318 — procsub `$!` + assignment-RHS fd lifetime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** flip the `procsub` bash-suite category to PASS by (1) making a process substitution set `$!` to its child and keep it waitable, and (2) keeping an assignment-RHS procsub's `/dev/fd/N` fd open past the assignment.

**Architecture:** Fix 1 — `realize` sets `shell.last_bg_pid`; `cleanup` returns the reaped `(pid, code)` and `drain_procsubs` records it in the v306 saved-status ring so `wait "$!"` resolves. Fix 2 — a `Shell::procsub_deferred` list holds assignment-RHS procsubs (moved out of the per-command drain by `run_assignment_list`), drained at the `process_line_in_sinks` / function-return scope boundary.

**Tech Stack:** Rust (huck-engine crate), bash-diff harnesses.

## Global Constraints

- **Issue:** [#218](https://github.com/jdstanhope/huck/issues/218). Spec: `docs/superpowers/specs/2026-07-20-procsub-lifetime-design.md`.
- bash 5.2.21 parity. Both fixes are required to flip `procsub`.
- Consuming-command procsubs (`cat <(…)`, `3< <(…)`) keep their current per-command drain — do NOT change that path (the `ulimit -n` fd-exhaustion test depends on it).
- Non-goal: bash's exact lazy `/dev/fd` reuse; the separate-top-level-statement `f=<(…)` / `cat $f` form (documented in the spec).
- **Box/build:** `cargo build -p huck --bin huck`; `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`; touched `-p huck` integration binaries at `--test-threads 2`. NEVER `cargo test --workspace`. `cargo fmt --all` before commit. `/usr/bin/grep` only. `BASH_SOURCE_DIR=/tmp/bash-5.2.21` for the runner.
- Commit trailer (exact): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## File structure

- `crates/huck-engine/src/procsub.rs` — `realize_via_devfd`/`realize_via_fifo` set `last_bg_pid`; `cleanup` returns `(pid, code)`.
- `crates/huck-engine/src/executor.rs` — `drain_procsubs`/`drain_procsubs_nonblocking` record the ring; `run_assignment_list` moves its procsubs to `procsub_deferred`; a `drain_deferred_procsubs` helper.
- `crates/huck-engine/src/shell_state.rs` — `Shell::procsub_deferred` field.
- `crates/huck-engine/src/shell.rs` — `process_line_in_sinks` drains the deferred scope.
- `tests/scripts/procsub_lifetime_diff_check.sh` — NEW harness.

---

### Task 1: `$!` from a procsub + saved-status ring

**Files:**
- Modify: `crates/huck-engine/src/procsub.rs` (`realize_via_devfd` ~106, `realize_via_fifo`, `cleanup` ~end)
- Modify: `crates/huck-engine/src/executor.rs` (`drain_procsubs` ~4061, `drain_procsubs_nonblocking` ~4075)
- Test: `tests/scripts/procsub_lifetime_diff_check.sh` (created here; extended in Task 3)

**Interfaces:**
- Produces: `procsub::cleanup(ps: ProcSub) -> Option<(i32, i32)>` (reaped pid + decoded code; `None` if nothing reaped).

- [ ] **Step 1: Write the failing harness** `tests/scripts/procsub_lifetime_diff_check.sh` (model on `eval_line_diag_diff_check.sh`; capture stdout+stderr merged AND rc without a pipe; normalize the name prefix). Task 1 adds the `$!` cases; Task 2 adds the lifetime cases:
```bash
#!/usr/bin/env bash
# v318 (#218): process-substitution $! + assignment-RHS fd lifetime.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck): #SH: #"; }
check() { local l=$1 f=$2 b h br hr
  b=$(bash -c "$f" 2>&1); br=$?; b=$(printf '%s' "$b" | norm)
  h=$("$HUCK" -c "$f" 2>&1); hr=$?; h=$(printf '%s' "$h" | norm)
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then echo "FAIL [$l]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1; else echo "PASS [$l]"; fi; }
# --- Fix 1: $! from a process substitution
check 'bang-wait-status'  'cat <(exit 123) >/dev/null; wait "$!"; echo $?'
check 'bang-is-set'       'cat <(:) >/dev/null; [ -n "$!" ] && echo set || echo unset'
# --- control: $! from a real background job still works (last-writer-wins)
check 'bang-real-bg'      'cat <(:) >/dev/null; sleep 0 & p=$!; wait "$p"; echo "$?"'
if [ $FAIL -ne 0 ]; then echo "procsub_lifetime_diff_check FAILED" >&2; exit 1; fi
echo "procsub_lifetime_diff_check OK"
```
`chmod +x`. Run `cargo build -p huck --bin huck && bash tests/scripts/procsub_lifetime_diff_check.sh` — `bang-wait-status` FAILS (huck `$!` empty → wait errors), `bang-is-set` FAILS, `bang-real-bg` may pass. That's the red state.

- [ ] **Step 2: `realize` sets `$!`.** In `procsub.rs`, in BOTH `realize_via_devfd` and `realize_via_fifo`, immediately after `fork_and_run_in_subshell` returns `pid` (before building the `ProcSub`), add:
```rust
    // bash: a process substitution sets $! to its child's PID. v318 (#218).
    shell.last_bg_pid = Some(pid);
```
(Both functions already take `shell: &mut Shell`.)

- [ ] **Step 3: `cleanup` returns the reaped status.** Change `cleanup`:
```rust
/// Close the parent fd, unlink any FIFO, and reap the inner child. Returns the
/// reaped `(pid, decoded_code)` so the caller can record it in the saved-status
/// ring (v306), letting a later `wait "$!"` resolve. `None` if nothing reaped.
pub fn cleanup(ps: ProcSub) -> Option<(i32, i32)> {
    if ps.parent_fd >= 0 {
        unsafe { libc::close(ps.parent_fd); }
    }
    if let Some(p) = &ps.fifo_path {
        let _ = std::fs::remove_file(p);
    }
    if ps.pid <= 0 {
        return None;
    }
    let mut status = 0;
    let r = unsafe { libc::waitpid(ps.pid, &mut status, 0) };
    if r <= 0 {
        return None;
    }
    let code = if unsafe { libc::WIFEXITED(status) } {
        unsafe { libc::WEXITSTATUS(status) }
    } else if unsafe { libc::WIFSIGNALED(status) } {
        128 + unsafe { libc::WTERMSIG(status) }
    } else {
        0
    };
    Some((ps.pid, code))
}
```

- [ ] **Step 4: Record in the ring at the drain sites.** In `executor.rs`, `drain_procsubs`:
```rust
fn drain_procsubs(shell: &mut Shell, base: usize) {
    while shell.procsub_pending.len() > base {
        if let Some(ps) = shell.procsub_pending.pop() {
            if let Some((pid, code)) = crate::procsub::cleanup(ps) {
                shell.jobs.record_terminal_status(pid, code);
            }
        }
    }
}
```
In `drain_procsubs_nonblocking`: it does its own inline close + `WNOHANG` waitpid (it does NOT call `cleanup`). Leave its non-blocking behavior, but when its `waitpid(..., WNOHANG)` returns `> 0`, record the decoded status too:
```rust
            let mut status = 0;
            let r = unsafe { libc::waitpid(ps.pid, &mut status, libc::WNOHANG) };
            if r > 0 {
                let code = if unsafe { libc::WIFEXITED(status) } {
                    unsafe { libc::WEXITSTATUS(status) }
                } else if unsafe { libc::WIFSIGNALED(status) } {
                    128 + unsafe { libc::WTERMSIG(status) }
                } else { 0 };
                shell.jobs.record_terminal_status(ps.pid, code);
            }
```
(Any OTHER caller of `procsub::cleanup` — grep `procsub::cleanup` — must be updated for the new return type; if a site has no `&mut shell` in scope, discard the result with `let _ =`.)

- [ ] **Step 5: Build + verify Fix-1 harness cases green.** `cargo build -p huck --bin huck && bash tests/scripts/procsub_lifetime_diff_check.sh` — `bang-wait-status`/`bang-is-set`/`bang-real-bg` all PASS. `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` green. Manually confirm `./target/debug/huck -c 'cat <(exit 123) >/dev/null; wait "$!"; echo $?'` prints `123` (matches bash).
- [ ] **Step 6: fmt + commit** (`v318 task 1: process substitution sets $! + saves child status to the wait ring (#218)`).

---

### Task 2: assignment-RHS procsub fd lifetime (deferred list)

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (add `procsub_deferred` field + init)
- Modify: `crates/huck-engine/src/executor.rs` (`run_assignment_list` moves its procsubs; `drain_deferred_procsubs` helper)
- Modify: `crates/huck-engine/src/shell.rs` (`process_line_in_sinks` drains the deferred scope)

**Interfaces:**
- Consumes: Task 1's `cleanup` return + ring recording.
- Produces: `Shell::procsub_deferred: Vec<crate::procsub::ProcSub>`; `drain_deferred_procsubs(shell, base)`.

- [ ] **Step 1: Add the lifetime harness cases** to `tests/scripts/procsub_lifetime_diff_check.sh` (they FAIL now — `/dev/fd/N: Permission denied`):
```bash
# --- Fix 2: assignment-RHS fd lifetime
check 'assign-then-read'  'eval f=<(echo test4) "; cat \$f"'
check 'assign-plain'      'f=<(echo hi); cat "$f"'
# --- control: consuming-command procsub still works (per-command drain)
check 'consume-two'       'cat <(echo a) <(echo b)'
check 'consume-func'      'f2(){ cat "$1"; }; f2 <(echo x)'
```
Run — `assign-then-read`/`assign-plain` FAIL, controls PASS.

- [ ] **Step 2: Add the `procsub_deferred` field.** In `shell_state.rs`, next to `procsub_pending`:
```rust
    /// Process substitutions realized in a standalone-assignment RHS
    /// (`f=<(…)`), whose `/dev/fd/N` path escapes into a variable. Not drained by
    /// the per-command drain; drained at the enclosing scope boundary
    /// (`process_line_in_sinks` / function return). v318 (#218).
    pub procsub_deferred: Vec<crate::procsub::ProcSub>,
```
Initialize `procsub_deferred: Vec::new(),` in the `Shell` constructor (near `procsub_pending`).

- [ ] **Step 3: `run_assignment_list` moves its procsubs to the deferred list.** In `executor.rs` `run_assignment_list` (~4105): snapshot at entry, move at the single return. At the top of the function body (after the existing "reset so only THESE assignments' RHS command substitutions count" logic — read it; there is already a procsub-base reset comment there):
```rust
    let procsub_base = shell.procsub_pending.len();
```
Just before EVERY `return`/at the final expression of `run_assignment_list`, move the tail. Since the function returns an `i32` (status) in a few places, wrap the body so the move runs on all exits — simplest is to compute the status into a local and move before returning it:
```rust
    // v318 (#218): a standalone assignment's RHS procsubs escape into a variable
    // (f=<(…)); defer their cleanup past this command so a later `cat $f` works.
    let deferred: Vec<_> = shell.procsub_pending.drain(procsub_base..).collect();
    shell.procsub_deferred.extend(deferred);
    status
```
(Read the real control flow first; if `run_assignment_list` has multiple `return`s, refactor to a single tail move — the function is short. Confirm no procsub from these assignments is left in `procsub_pending`, so the enclosing `run_exec_single` drain is a no-op for them.)

- [ ] **Step 4: Add `drain_deferred_procsubs`.** In `executor.rs`, next to `drain_procsubs`:
```rust
/// Drain deferred (assignment-RHS) process substitutions realized since `base`,
/// recording each reaped child's status in the saved-status ring. Called at the
/// input-unit / function scope boundary. v318 (#218).
pub(crate) fn drain_deferred_procsubs(shell: &mut Shell, base: usize) {
    while shell.procsub_deferred.len() > base {
        if let Some(ps) = shell.procsub_deferred.pop() {
            if let Some((pid, code)) = crate::procsub::cleanup(ps) {
                shell.jobs.record_terminal_status(pid, code);
            }
        }
    }
}
```

- [ ] **Step 5: Drain at `process_line_in_sinks` boundary.** In `shell.rs` `process_line_in_sinks`, snapshot the deferred length at entry and drain on the return. Since the function has one main `match parse_sequence {…}` returning an `ExecOutcome`, capture the base at the top:
```rust
    let deferred_base = shell.procsub_deferred.len();
```
and wrap the result so the drain runs on all exit paths — compute the outcome into a local, drain, then return it:
```rust
    let outcome = match parser::parse_sequence(&mut lx) { … };  // existing body
    crate::executor::drain_deferred_procsubs(shell, deferred_base);
    outcome
```
(If the existing body has early `return`s, convert them to producing the local `outcome`, or add the drain before each. The `eval "f=<(…); cat $f"` case: the whole list is one `process_line_in_sinks` call, so the deferred fd lives across the assignment AND `cat`, and is closed here after both.)

- [ ] **Step 6: (Best-effort) drain at function return.** The `procsub.tests` flip does NOT need this — its assignment-RHS cases are all top-level, so the `process_line_in_sinks` drain (Step 5) already bounds their lifetime, and a function's deferred procsub is bounded by the enclosing input unit's drain anyway. IF the function-invocation site (the executor path that runs a function-body `Sequence` and restores the caller's local scope — grep the executor for where a user function's body is dispatched / locals are unwound) is clear from reading the code, snapshot `procsub_deferred.len()` before the body and `drain_deferred_procsubs(shell, base)` after it on all exit paths (so `f(){ x=<(…); }` cleans up at return, matching bash's function scope). If the site is NOT obvious without deep spelunking, SKIP it for v318 and note it in the report as a bounded follow-on (the input-unit drain still prevents any unbounded leak) — do not block the flip on it.

- [ ] **Step 7: Build + verify Fix-2 harness cases green.** `cargo build -p huck --bin huck && bash tests/scripts/procsub_lifetime_diff_check.sh` — ALL cases (Fix 1 + Fix 2 + controls) PASS. `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` green. Confirm `./target/debug/huck -c 'eval f=<(echo test4) "; cat \$f"'` prints `test4`.
- [ ] **Step 8: fmt + commit** (`v318 task 2: defer assignment-RHS procsub cleanup to the scope boundary (#218)`).

---

### Task 3: procsub flip + sweep + close #218

**Files:**
- Modify: `docs/bash-test-suite-baseline.md` (PASS 16 → 17, procsub → PASS)

- [ ] **Step 1: Confirm procsub flips.** Build release: `cargo build --release -p huck --bin huck`. Then `export BASH_SOURCE_DIR=/tmp/bash-5.2.21; HUCK_BASH_TEST_CATEGORY=procsub timeout 120 bash tests/bash-test-suite/runner.sh 2>&1 | /usr/bin/grep -E '\| procsub \||PASS:|FAIL:'`. Expected: **procsub PASS** (0-diff). If it still FAILs, read the newest `/tmp/huck-bash-tests-*/procsub.diff` and report the residual — do NOT hand-wave.
- [ ] **Step 2: Regression — procsub/process-substitution coverage.** Run any `-p huck` integration binary touching procsub/`<(` at `--test-threads 2` (grep `tests/*.rs` for `procsub|proc_sub|<(`); run the existing procsub bash-diff harnesses. All green — the deferred list must not leak fds or reap the wrong child.
- [ ] **Step 3: Full lib + sweep.** `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` green; build debug+release; `ulimit -v 1500000; timeout 900 bash tests/scripts/run_diff_checks.sh 2>&1 | tail -6` green (`coproc_diff_check.sh` known-flake note as usual — but v317 made it reliable, so it should pass).
- [ ] **Step 4: Baseline doc.** Bump Summary PASS 16 → 17, FAIL 66 → 65; set the `procsub` row to PASS with a note ("v318 (#218): `$!` from a procsub + assignment-RHS fd lifetime — resolved"); refresh the provenance line. Only claim procsub flipped.
- [ ] **Step 5: fmt + commit** (`v318 task 3: procsub flips to PASS + baseline (#218)`). The merged PR closes #218 via `Closes #218`.

---

## Notes for the executor

- Fix 1 and Fix 2 both reap procsub children and record to the saved-status ring — verify no DOUBLE-reap (a procsub is in EITHER `procsub_pending` (consuming) OR `procsub_deferred` (assignment), never both; `run_assignment_list` moves it out of pending before the enclosing drain).
- `$!` "last writer wins": a real `sleep &` after a procsub overwrites `last_bg_pid` — that's bash-correct (the `bang-real-bg` control pins it).
- Do NOT change the consuming-command drain path (`cat <(…)` etc.) — only assignment-RHS procsubs defer.
