# Job-control Stopped/Running state reflection (#158) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `kill -STOP` / `kill -s CONT` correctly reflect a job's Running/Stopped state in `jobs`/`jobs -s`/`jobs -r`/`bg`/`fg` in non-interactive mode, matching bash.

**Architecture:** huck already has the full `JobState::{Running,Stopped,Done}` model, `Stopped` rendering, and a `WUNTRACED` reap (`reap_completed`) that transitions to `Stopped` — but that reap only runs in the interactive REPL and the `wait` builtin, so non-interactive `jobs`/`bg`/`fg` never observe the stop. Fix: (1) add `WCONTINUED` + a `WIFCONTINUED` arm so continues are observed; (2) call `reap_completed(shell)` at the entry of `builtin_jobs`/`bg`/`fg`.

**Tech Stack:** Rust, `crates/huck-engine/src/jobs.rs` + `builtins.rs`, a bash-diff harness.

**Design spec:** `docs/superpowers/specs/2026-07-15-job-stop-cont-state-design.md`. Refs [#158](https://github.com/jdstanhope/huck/issues/158). **Scope (a) only** — NOT async job notifications (b) NOR job-spec gaps (c).

## Global Constraints

- **Commit trailer** verbatim on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **`cargo fmt --all`** before every commit (CI runs `--check`).
- **Build:** `cargo build -p huck` (produces `target/debug/huck`). **Never** `cargo test --workspace` (OOMs this box). Engine lib tests: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
- **Scope discipline:** do NOT implement async job-status notifications or job-spec (`%job`-as-command / `%?substr`) changes. Those are deferred follow-ups off #158.
- **Consequence to keep in mind:** the full `jobs` bash-suite category stays FAIL after this (its async notices still diff). The gate is the targeted state-only harness + unit tests, NOT the `jobs` category flipping to PASS.

---

### Task 1: State machine — observe `WCONTINUED`, transition Stopped→Running

**Files:**
- Modify: `crates/huck-engine/src/jobs.rs` (`reap_completed` flags ~line 304; `reap()` continued arm ~line 142; unit tests near the existing `reap_with_stopped_status_*` tests ~line 666)

**Interfaces:**
- Consumes: `libc::{WCONTINUED, WIFCONTINUED, WNOHANG, WUNTRACED, WIFSTOPPED}`; `JobState::{Running, Stopped}`; the existing `JobTable::{add, reap, iter, jobs_mut}` and test helpers `fake_done_raw`/`fake_signaled_raw`.
- Produces: `reap()` now flips a `Stopped` job to `Running` on a `WIFCONTINUED` status; `reap_completed` observes continued children.

- [ ] **Step 1: Write the failing unit tests**

In `crates/huck-engine/src/jobs.rs`, in the `#[cfg(test)] mod` (next to `reap_with_stopped_status_transitions_job_to_stopped_state`, ~line 666), add a continued-status helper and two tests:

```rust
    // A raw waitpid status for which WIFCONTINUED is true. On Linux/glibc
    // WIFCONTINUED(status) is `status == 0xffff` (the __W_CONTINUED sentinel).
    fn fake_continued_raw() -> libc::c_int {
        0xffff
    }

    #[test]
    fn reap_continued_transitions_stopped_job_to_running() {
        let mut t = JobTable::new();
        let _ = t.add(4242, vec![4242], "sleep 100".to_string());
        // First stop it.
        let stopped: libc::c_int = (libc::SIGTSTP << 8) | 0x7f;
        t.reap(4242, stopped);
        assert!(matches!(t.jobs_mut()[0].state, JobState::Stopped(_)));
        // A continued report flips it back to Running (not reaped/Done).
        t.reap(4242, fake_continued_raw());
        let j = &t.jobs_mut()[0];
        assert!(
            matches!(j.state, JobState::Running),
            "continued job must be Running, got {:?}",
            j.state
        );
        assert!(!j.reaped[0], "a continue is not a terminal reap");
        assert!(!j.notified, "a resumed job must be visible to the next pass");
    }

    #[test]
    fn reap_continued_on_running_job_is_noop() {
        let mut t = JobTable::new();
        let _ = t.add(4242, vec![4242], "sleep 100".to_string());
        t.reap(4242, fake_continued_raw());
        assert!(matches!(t.jobs_mut()[0].state, JobState::Running));
        assert!(!t.jobs_mut()[0].reaped[0]);
    }
```

- [ ] **Step 2: Run the tests to confirm they fail (RED)**

Run: `cargo test -p huck-engine --jobs 1 --lib reap_continued -- --test-threads 1`
Expected: FAIL — before the fix, `reap()` has no `WIFCONTINUED` arm, so the continued status (`0xffff`) falls through to the normal reap path (`0xffff` low byte `0xff` is `WIFSTOPPED`-ish / not `WIFEXITED`), leaving state wrong or panicking on `reaped` indexing. (Either a wrong-state assert or a fall-through — the point is it is not `Running`.)

- [ ] **Step 3: Add the `WIFCONTINUED` arm in `reap()`**

In `crates/huck-engine/src/jobs.rs`, in `reap()`, immediately AFTER the existing `WIFSTOPPED` block's closing `return;` (currently line 141-142, right before `if job.reaped[idx] {`), insert:

```rust
                if libc::WIFCONTINUED(raw_status) {
                    // A WCONTINUED report: a previously-Stopped job resumed
                    // (e.g. `kill -s CONT` / `bg`). Flip it back to Running. A
                    // continue is NOT a terminal reap, so do not touch
                    // `job.reaped[idx]`. Idempotent: no-op if already Running.
                    if matches!(job.state, JobState::Stopped(_)) {
                        job.state = JobState::Running;
                        job.notified = false;
                    }
                    return;
                }
```

- [ ] **Step 4: Add `WCONTINUED` to the reap waitpid flags**

In `reap_completed` (~line 304), change:

```rust
        let pid = unsafe { libc::waitpid(-1, &mut raw_status, libc::WNOHANG | libc::WUNTRACED) };
```

to:

```rust
        let pid = unsafe {
            libc::waitpid(-1, &mut raw_status, libc::WNOHANG | libc::WUNTRACED | libc::WCONTINUED)
        };
```

- [ ] **Step 5: Confirm the unit tests pass (GREEN) and the suite is clean**

Run: `cargo test -p huck-engine --jobs 1 --lib reap_ -- --test-threads 1`
Expected: the two new tests PASS and the existing `reap_*` tests (single/pipeline/stopped/signaled) still PASS.

- [ ] **Step 6: Format and commit**

```bash
cargo fmt --all && cargo build -p huck
git add crates/huck-engine/src/jobs.rs
git commit -m "$(cat <<'EOF'
fix(#158): observe WCONTINUED so a resumed job returns to Running

reap_completed now waits with WCONTINUED, and reap() flips a Stopped job back
to Running on a WIFCONTINUED status (non-terminal; idempotent on a Running job).

Refs #158.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Wire the reap into the job-state readers + state-only diff-check

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (entry of `builtin_jobs` ~4242, `builtin_fg` ~5124, `builtin_bg` ~5219)
- Create: `tests/scripts/job_stop_cont_diff_check.sh`
- (No edit to `run_diff_checks.sh` — it auto-discovers `tests/scripts/*_diff_check.sh`.)

**Interfaces:**
- Consumes: `crate::jobs::reap_completed(shell)` (non-blocking, idempotent; takes `&mut Shell`); the readers already hold `shell: &mut Shell`.
- Produces: non-interactive `jobs`/`jobs -s`/`jobs -r`/`bg`/`fg` observe pending STOP/CONT before reading state.

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/job_stop_cont_diff_check.sh`:

```bash
#!/usr/bin/env bash
# v299 (#158): kill -STOP / kill -s CONT must update a job's Running/Stopped
# state as seen by jobs/jobs -s/jobs -r/bg — matching bash. Non-interactive
# reap wiring: builtin_jobs/bg/fg drain pending WUNTRACED/WCONTINUED reports.
#
# bash also emits ASYNC job notices under `set -m` even non-interactively
# (deferred scope (b), issue #158) — so this harness does NOT byte-compare the
# whole stream. Instead each fragment absorbs the async notice with an
# intervening `:` command, prints a `===` marker, then a jobs query; we compare
# only the post-marker query, normalized to `[id] <State>` (dropping the flag,
# spacing, and the command column — the command column is the separate #80).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Everything after the last `===` marker line, each job-status line reduced to
# `[<id>] <State>` (first word after the flag). Non-job lines are dropped.
post_marker_state() {
  sed '1,/^===$/d' | sed -nE 's/^(\[[0-9]+\])[-+ ]+ *([A-Za-z]+).*/\1 \2/p'
}
check() {
  local label=$1 frag=$2 b h
  b=$(timeout 20 bash    -c "$frag" 2>/dev/null | post_marker_state)
  h=$(timeout 20 "$HUCK" -c "$frag" 2>/dev/null | post_marker_state)
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: [$b]"; echo "  huck: [$h]"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- STOP reflected: jobs -s lists the stopped job, jobs -r does not ---
check 'stop-shows-stopped' 'set -m; sleep 30 & kill -STOP %1; sleep 1; :; echo ===; jobs -s; kill -9 %1 2>/dev/null'
check 'stop-not-running'   'set -m; sleep 30 & kill -STOP %1; sleep 1; :; echo ===; jobs -r; kill -9 %1 2>/dev/null'
# --- CONT reflected: jobs -r lists the resumed job, jobs -s does not ---
check 'cont-shows-running' 'set -m; sleep 30 & kill -STOP %1; sleep 1; :; kill -s CONT %1; sleep 1; :; echo ===; jobs -r; kill -9 %1 2>/dev/null'
check 'cont-not-stopped'   'set -m; sleep 30 & kill -STOP %1; sleep 1; :; kill -s CONT %1; sleep 1; :; echo ===; jobs -s; kill -9 %1 2>/dev/null'
# --- bg resumes a stopped job -> Running ---
check 'bg-resumes-running' 'set -m; sleep 30 & kill -STOP %1; sleep 1; :; bg %1 >/dev/null 2>&1; sleep 1; :; echo ===; jobs -r; kill -9 %1 2>/dev/null'
# --- plain `jobs` shows Stopped after STOP (state token only) ---
check 'stop-plain-jobs'    'set -m; sleep 30 & kill -STOP %1; sleep 1; :; echo ===; jobs; kill -9 %1 2>/dev/null'

if [ $FAIL -ne 0 ]; then echo "job_stop_cont_diff_check FAILED" >&2; exit 1; fi
echo "job_stop_cont_diff_check OK"
```

- [ ] **Step 2: Run it to confirm it fails (RED)**

```bash
cargo build -p huck && chmod +x tests/scripts/job_stop_cont_diff_check.sh && tests/scripts/job_stop_cont_diff_check.sh
```
Expected: FAIL on `stop-shows-stopped` / `stop-plain-jobs` / etc. — before the wiring, huck's `jobs -s` is empty and `jobs -r`/plain `jobs` show the job as Running (bash shows Stopped), because `builtin_jobs` never reaped the stop. (Task 1 is already merged, so the `cont-*` cases exercise WCONTINUED, but they also need the wiring to observe anything.)

- [ ] **Step 3: Reap at the entry of `builtin_jobs`**

In `crates/huck-engine/src/builtins.rs`, `builtin_jobs` (~4242), insert as the FIRST statement of the body (before `let parsed = match parse_jobs_args(...)`):

```rust
    // #158: observe any pending STOP/CONT reports before reading job state, so
    // non-interactive `jobs` reflects Stopped/Running like the interactive REPL
    // (which reaps pre-prompt). Non-blocking + idempotent.
    crate::jobs::reap_completed(shell);
```

- [ ] **Step 4: Reap at the entry of `builtin_fg` and `builtin_bg`**

In `builtin_fg` (~5124), insert as the FIRST statement (before `let id = match args.len()`):

```rust
    // #158: drain pending STOP/CONT before resolving/acting on the job.
    crate::jobs::reap_completed(shell);
```

In `builtin_bg` (~5219), insert as the FIRST statement of the body (after the multi-line signature's `{`, before `let id = match args.len()`):

```rust
    // #158: drain pending STOP/CONT so `bg` finds a newly-stopped job.
    crate::jobs::reap_completed(shell);
```

- [ ] **Step 5: Format, build, confirm the harness is GREEN**

```bash
cargo fmt --all && cargo build -p huck && tests/scripts/job_stop_cont_diff_check.sh
```
Expected: `job_stop_cont_diff_check OK` (all PASS).

- [ ] **Step 6: Confirm no job-control regressions**

```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
for h in tests/scripts/*job*_diff_check.sh tests/scripts/coproc*_diff_check.sh; do [ -x "$h" ] && "$h" 2>&1 | tail -1; done
```
Expected: engine lib green (~1807 with the two new tests); any existing job/coproc harness stays green.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/scripts/job_stop_cont_diff_check.sh
git commit -m "$(cat <<'EOF'
fix(#158): reflect STOP/CONT job state in non-interactive jobs/bg/fg

builtin_jobs/fg/bg now call reap_completed at entry, so `jobs`, `jobs -s/-r`,
and `bg` observe pending WUNTRACED/WCONTINUED reports and show a job as
Stopped/Running like the interactive REPL. New job_stop_cont_diff_check.sh
compares the state token only (bash's async set -m notices are deferred (b)).

Refs #158.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: whole-branch verification

**Files:** none (verification only).

- [ ] **Step 1: Build + fmt check**

```bash
cargo build -p huck && cargo fmt --all --check && echo "(fmt clean)"
```

- [ ] **Step 2: Engine lib tests**

```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
```
Expected: all pass.

- [ ] **Step 3: Full diff sweep (both binaries)**

```bash
cargo build --release --locked -p huck
( ulimit -v 1500000; timeout 1000 tests/scripts/run_diff_checks.sh 2>&1 | tail -5 )
```
Expected: 0 failed, and `job_stop_cont_diff_check.sh` appears in the run and passes.

- [ ] **Step 4: Sanity — the original #158 repro now matches bash on state**

```bash
target/debug/huck -c 'set -m; sleep 5 & kill -STOP %1; sleep 1; jobs -s; kill -9 %1' 2>/dev/null
```
Expected: a `[1]... Stopped ...` line (state correct; the argless command column is the separate #80).

---

## Self-Review

- **Spec coverage:** Section 1 (WCONTINUED flag + WIFCONTINUED arm) → Task 1. Section 2 (reap wiring in jobs/fg/bg) → Task 2. Testing (unit tests + state-isolating harness) → Tasks 1-2; regression sweep → Task 3. Scope boundary (no async notices, no job-spec) → Global Constraints + harness comment. Covered.
- **Placeholder scan:** every step has concrete code/commands and expected output; the harness is complete. No TBD/TODO.
- **Type consistency:** `reap_completed(&mut Shell)` used at three call sites that hold `shell: &mut Shell`; `WIFCONTINUED`/`WCONTINUED` are `libc` items (verified present); `fake_continued_raw() -> libc::c_int = 0xffff`; `JobState::{Stopped,Running}` matched as elsewhere in the file.
- **Sequencing:** Task 1 (state machine + WCONTINUED) precedes Task 2 (wiring + harness) because the `cont-*` harness cases need `WIFCONTINUED`. Task 2's RED is still meaningful for the STOP cases even with Task 1 merged.
- **Risk:** `reap_completed` at reader entry is non-blocking/idempotent and only mutates `job.state`; it does not print (that's `reap_and_notify`), so no async-notification behavior leaks in. Confirm no existing test asserts that `jobs`/`bg`/`fg` do NOT reap (none expected).
