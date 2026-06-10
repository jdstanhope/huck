# v133 — drain a captured pipeline concurrently (M-119) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the deadlock where `x="$(producer | filter)"` hangs once the captured output exceeds the OS pipe buffer — the remaining `nvm ls-remote` blocker (M-119).

**Architecture:** In `run_multi_stage` (src/executor.rs), drain the capture pipe (`io::copy` of `capture_read_fd` into the substitution buffer) BEFORE `wait_pipeline_raw` instead of after — mirroring the single-command capture path (executor.rs:378). The drain reads to EOF, overlapping with the (separate-process) stages writing. Safe because a Capture sink ⟹ `interactive == false`, so the terminal-handoff / stopped-pipeline blocks are no-ops in that case.

**Tech Stack:** Rust, libc/unix pipes. Tests: cargo integration with a watchdog-thread timeout guard + a bash-diff harness + a real `nvm ls-remote` PTY payoff.

**GIT SAFETY:** Do NOT `git checkout <sha>` — stay on `v133-captured-pipeline-drain`; edit, build, commit in place. Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.

**Reference:** spec `docs/superpowers/specs/2026-06-10-captured-pipeline-drain-design.md`. Key locations in `run_multi_stage` (src/executor.rs:3493+): `interactive` at :3505 (`matches!(sink, StdoutSink::Terminal) && …`); parent-held-fd cleanup ending `parent_held.retain(|&fd| Some(fd) == capture_read_fd)` (~:4095); `if interactive && let Some(pgid) = first_pid { give_terminal_to(pgid); }` (~:4097-4100); `wait_pipeline_raw(...)` (~:4103); the stopped-pipeline early-return closing `capture_read_fd` (~:4112); the post-wait capture read `if let Some(r) = capture_read_fd.take() { … io::copy … }` (~:4117-4126). The single-command reference drain is at executor.rs:378-384.

---

### Task 1: Drain-before-wait + integration tests

**Files:**
- Create: `tests/captured_pipeline_drain_integration.rs`
- Modify: `src/executor.rs` (relocate the capture drain in `run_multi_stage`)

- [ ] **Step 1: Write the failing (hang-guarded) integration tests** — create `tests/captured_pipeline_drain_integration.rs`:

```rust
//! v133: a captured pipeline whose output exceeds the pipe buffer must not
//! deadlock (M-119). Each run is wrapped in a watchdog that kills the child
//! after a timeout, so a regression FAILS as a timeout rather than hanging the
//! test run.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Runs `script` through huck with a `secs` watchdog. Returns
/// `Some((stdout, stderr, code))` on normal completion, `None` if it hung
/// (the watchdog had to SIGKILL it).
fn run_guarded(script: &str, secs: u64) -> Option<(String, String, i32)> {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    let pid = child.id();
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    // Watchdog: if not told to stop within `secs`, SIGKILL the child by pid.
    let (tx, rx) = mpsc::channel::<()>();
    let wd = thread::spawn(move || -> bool {
        if rx.recv_timeout(Duration::from_secs(secs)).is_err() {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            true // had to kill => hang
        } else {
            false
        }
    });
    let out = child.wait_with_output().unwrap();
    let _ = tx.send(()); // tell the watchdog to stop (no-op if it already fired)
    let killed = wd.join().unwrap();
    if killed {
        None
    } else {
        Some((
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
            out.status.code().unwrap_or(-1),
        ))
    }
}

#[test]
fn large_captured_pipeline_does_not_hang() {
    let r = run_guarded("x=$(seq 1 500000 | cat); echo ${#x}\n", 10);
    let (o, _e, _c) = r.expect("HUNG: captured large pipeline deadlocked");
    assert_eq!(o.trim(), "3388894", "o: {o:?}");
}

#[test]
fn three_stage_captured_pipeline() {
    let r = run_guarded("x=$(seq 1 200000 | cat | cat); echo ${#x}\n", 10);
    let (o, _e, _c) = r.expect("HUNG: 3-stage captured pipeline deadlocked");
    // seq 1 200000 = digits 1..200000; assert it's the full length, not truncated.
    let expected = (1..=200000).map(|n| n.to_string().len() + 1).sum::<usize>();
    assert_eq!(o.trim(), expected.to_string(), "o: {o:?}");
}

#[test]
fn small_captured_pipeline_still_works() {
    let r = run_guarded("x=$(seq 1 1000 | cat); echo ${#x}\n", 10);
    let (o, _e, _c) = r.expect("hung");
    assert_eq!(o.trim(), "3892", "o: {o:?}");
}

#[test]
fn large_producer_small_final_output() {
    let r = run_guarded("x=$(seq 1 500000 | wc -l); echo \"[$x]\"\n", 10);
    let (o, _e, _c) = r.expect("hung");
    assert_eq!(o.trim(), "[500000]", "o: {o:?}");
}

#[test]
fn non_capture_pipeline_unaffected() {
    let r = run_guarded("seq 1 100 | wc -l\n", 10);
    let (o, _e, _c) = r.expect("hung");
    assert_eq!(o.trim(), "100", "o: {o:?}");
}

#[test]
fn pipestatus_after_captured_pipeline() {
    // Verify $PIPESTATUS is unaffected by the relocation. Compare to bash:
    // bash: `x=$(false | true); echo "${PIPESTATUS[@]}"` -> "0" (the assignment's
    // own pipeline status; the inner pipeline's PIPESTATUS is scoped to the subst).
    let r = run_guarded("x=$(false | true); echo \"[${PIPESTATUS[*]}]\"\n", 10);
    let (o, _e, _c) = r.expect("hung");
    // Assert it matches bash's output for THIS exact fragment — the implementer
    // must run bash to confirm the expected string, then hard-code it here.
    assert_eq!(o.trim(), "[0]", "o: {o:?} — if bash differs, set the expected to bash's output");
}
```

- [ ] **Step 2: Run to verify the hang is caught** — `cargo test --test captured_pipeline_drain_integration 2>&1 | tail -25`. Expected (BEFORE the fix): `large_captured_pipeline_does_not_hang` and `three_stage_captured_pipeline` FAIL via the watchdog (`None` → `.expect(...)` panics "HUNG"); the small/large-producer/non-capture/pipestatus tests PASS. (The watchdog means a regression FAILS in ~10s, it does not hang the suite.) NOTE: if `pipestatus_after_captured_pipeline`'s expected `[0]` doesn't match bash, run `bash -c 'x=$(false | true); echo "[${PIPESTATUS[*]}]"'` and set the assertion to bash's exact output.

- [ ] **Step 3: Read the relocation site.** Read `src/executor.rs` ~4084-4126 to see the exact current order: (a) parent-held-fd cleanup ending `parent_held.retain(...)`; (b) `if interactive && let Some(pgid) = first_pid { give_terminal_to(pgid); }`; (c) `let last_status = wait_pipeline_raw(...)`; (d) the interactive stopped-pipeline block (closes `capture_read_fd` on early return); (e) the post-wait `if let Some(r) = capture_read_fd.take() { … io::copy … }`.

- [ ] **Step 4: Relocate the drain to BEFORE the wait.** Move the post-wait capture-read block (e) so it runs immediately AFTER the `parent_held.retain(|&fd| Some(fd) == capture_read_fd);` line (a) and BEFORE the `if interactive && let Some(pgid) = first_pid { give_terminal_to(pgid); }` block (b). It should read:
```rust
    parent_held.retain(|&fd| Some(fd) == capture_read_fd);

    // Drain the capture pipe BEFORE waiting (M-119): the final stage blocks on
    // write() once it fills the pipe buffer, so nothing draining during the wait
    // deadlocks. Reading to EOF here overlaps with the stages writing. Capture
    // sink ⟹ interactive == false, so the terminal-handoff/stopped blocks below
    // are no-ops in this case.
    if let Some(r) = capture_read_fd.take() {
        if let StdoutSink::Capture(buf) = sink {
            let mut f = unsafe { File::from_raw_fd(r) };
            let _ = io::copy(&mut f, *buf);
            // f drops here, closing r.
        } else {
            unsafe { libc::close(r); }
        }
    }

    // Give the terminal to the pipeline's process group if interactive.
    if interactive && let Some(pgid) = first_pid {
        give_terminal_to(pgid);
    }

    // ---- Wait for all stages ----
    let last_status = wait_pipeline_raw(&pipeline_stages, &stage_pids, first_pid, shell, interactive);
    ...
```
Then DELETE the old post-wait capture-read block (e). Because `capture_read_fd` is now `None` after the `.take()`, the stopped-pipeline early-return's `if let Some(r) = capture_read_fd { unsafe { libc::close(r); } }` (d) becomes dead — leave it (harmless `None` no-op) OR remove it for clarity; if you remove it, confirm the early return path still compiles. Do NOT change anything else in the wait/stopped/interactive logic.

Confirm `File`, `FromRawFd`, `io`, `libc` are already imported in scope at the new location (they were used by the old block — same function, so yes).

- [ ] **Step 5: Run the integration tests** — `cargo test --test captured_pipeline_drain_integration 2>&1 | tail -20`. Expected: all 6 pass (the two hang tests now complete).

- [ ] **Step 6: Build + FULL regression + clippy** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|panicked|test result: FAILED" | head` (none — ESPECIALLY pipeline/`$PIPESTATUS`/pipefail/job-control tests must stay green); `cargo test --test pty_interactive --test subshell_pipeline_pty --test subshell_tty_pty 2>&1 | tail -8` (interactive pipeline suites green); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 7: Sanity vs bash** (report):
```
for f in 'x=$(seq 1 500000 | cat); echo ${#x}' 'x=$(seq 1 200000 | cat | cat); echo ${#x}' 'x=$(seq 1 500000 | wc -l); echo [$x]'; do
  b=$(timeout 10 bash -c "$f" 2>&1); h=$(timeout 10 ./target/debug/huck -c "$f" 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF/HANG: $f"; diff <(echo "$b") <(echo "$h"); }
done
```

- [ ] **Step 8: Commit**
```bash
git add src/executor.rs tests/captured_pipeline_drain_integration.rs
git commit -m "$(cat <<'EOF'
fix(v133): drain a captured pipeline before waiting (M-119 deadlock)

run_multi_stage drained the capture pipe only AFTER wait_pipeline_raw, so a
captured pipeline whose output exceeds the ~64 KB pipe buffer deadlocked (the
final stage blocks on write, nothing reads, the wait never returns). Relocate the
drain to BEFORE the wait, mirroring the single-command capture path — reading to
EOF overlaps with the stages writing. Safe: capture sink => interactive == false,
so the terminal-handoff/stopped blocks are no-ops. Fixes x=$(seq 1 500000 | cat)
and the nvm ls-remote hang.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Bash-diff harness + nvm payoff + docs (resolve M-119)

**Files:**
- Create: `tests/scripts/captured_pipeline_drain_diff_check.sh`
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Bash-diff harness** — create `tests/scripts/captured_pipeline_drain_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v133: a captured pipeline larger than the
# pipe buffer must not deadlock (M-119). Each fragment is wrapped in `timeout` so a
# regression shows as a FAIL (non-zero exit / truncated output), not a hung harness.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 15 bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$(timeout 15 "$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "large captured pipe"   'x=$(seq 1 500000 | cat); echo ${#x}'
check "three-stage captured"  'x=$(seq 1 200000 | cat | cat); echo ${#x}'
check "small captured pipe"   'x=$(seq 1 1000 | cat); echo ${#x}'
check "large producer small"  'x=$(seq 1 500000 | wc -l); echo "[$x]"'
check "pipe tr filter large"  'x=$(seq 1 500000 | tr -d "\n" | wc -c); echo "[$x]"'
check "non-capture pipe"      'seq 1 100 | wc -l'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/captured_pipeline_drain_diff_check.sh`.

- [ ] **Step 2: Run the harness** — `cargo build 2>&1 | tail -2`; `bash tests/scripts/captured_pipeline_drain_diff_check.sh` → expect `Fail: 0`. Report any diff.

- [ ] **Step 3: nvm ls-remote payoff (REQUIRED — needs network; the v132 lesson: VERIFY before claiming).** Write a small python `pty.fork` harness that spawns interactive huck under a PTY, sends `source ~/.nvm/nvm.sh` (do NOT source `~/.bashrc` — it has PG* creds), then `nvm ls-remote`, then a sentinel `echo DONE_$((6*7))`, with a ~60s timeout. Report: did the sentinel arrive (no hang)? did version lines (e.g. `v` lines) appear? Also run the local nvm-shaped synthetic `timeout 10 ./target/debug/huck -c 'x=$(eval "seq 1 500000" | cat); echo ${#x}'` and confirm it completes. If there is genuinely NO network (curl fails fast), say so explicitly and rely on the synthetic + nvm-shaped local repro. Clean up the temp python file. DO NOT claim nvm ls-remote works unless the sentinel actually arrived.

- [ ] **Step 4: Delete M-119 from `docs/bash-divergences.md`.** Find `### M-119` (Tier-1). Delete the entire entry (resolved divergences are removed, not flipped). Decrement the Tier-1 count in the summary table (`| Bugs (Tier 1) | 2 | … (M-114, M-119). |` → `| Bugs (Tier 1) | 1 | … (M-114). |`). Search the doc for any other reference to M-119 and update. Do NOT touch M-114 or other entries.

- [ ] **Step 5: Verify docs** — `grep -n "M-119" docs/bash-divergences.md` → no matches; `grep -n "Bugs (Tier 1) | 1" docs/bash-divergences.md` → present.

- [ ] **Step 6: Full regression + clippy** — `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean); smoke `bash tests/scripts/eval_source_sink_diff_check.sh | tail -1` (the v132 harness still green).

- [ ] **Step 7: Commit**
```bash
git add tests/scripts/captured_pipeline_drain_diff_check.sh docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
test+docs(v133): captured-pipeline-drain harness; resolve M-119

Add the bash-diff harness (large captured pipelines + non-capture control) and
delete the now-fixed M-119 divergence (Tier-1 2->1). nvm ls-remote no longer hangs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-review notes
- **Spec coverage:** Task 1 = the one relocation + the hang-guarded integration tests (incl. `$PIPESTATUS` + non-capture no-regress); Task 2 = harness + the REQUIRED nvm payoff + delete M-119.
- **Type/symbol consistency:** the relocated block uses `capture_read_fd` / `StdoutSink::Capture(buf)` / `File::from_raw_fd` / `io::copy` — all already in scope in `run_multi_stage`. `wait_pipeline_raw` signature unchanged.
- **No-regress:** the relocated block is a no-op when `capture_read_fd` is `None` (every Terminal-sink pipeline) → ordinary/interactive pipelines unchanged; the watchdog test helper makes a regression a timeout-FAIL, not a suite hang.
- **Honesty gate:** Task 2 Step 3 REQUIRES the real `nvm ls-remote` to complete (sentinel arrives) before claiming the fix — the v132 lesson.
