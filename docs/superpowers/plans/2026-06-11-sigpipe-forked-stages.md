# huck v137 — SIGPIPE default disposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore the OS-default `SIGPIPE` disposition (`SIG_DFL`) process-wide so huck and the stages it forks die silently (status 141) on a broken pipe, like bash, instead of looping on `EPIPE` and spamming `huck: printf: Broken pipe`.

**Architecture:** Rust's runtime sets `SIGPIPE` to `SIG_IGN` at startup. Three small signal edits: (1) reset `SIGPIPE`→`SIG_DFL` once at shell startup in `install_job_control_signals()` (the fix; forked children inherit it); (2) an explicit `SIG_DFL` reset in the `fork_and_run_in_subshell` child (belt-and-suspenders + correct subshell-resets-traps semantics); (3) restore `SIG_IGN` in the `spawn_heredoc_writer` child so its v134 manual EPIPE handling is preserved byte-for-byte. No `builtins.rs` change. Verified by a bash-diff harness and binary-level integration tests.

**Tech Stack:** Rust, `libc` (raw `signal(2)`), the huck test binary (`env!("CARGO_BIN_EXE_huck")`), bash-diff harness shell scripts.

**Reference:** spec at `docs/superpowers/specs/2026-06-11-sigpipe-forked-stages-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a detached HEAD lost commits in a prior iteration). Stay on the `v137-sigpipe-forked-stages` branch. Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Build note:** the repo is large; `cargo build`/`cargo test` can take a few minutes. Integration tests spawn the huck binary, so `cargo test` rebuilds it automatically.

---

### Task 1: Core fix — reset SIGPIPE→SIG_DFL at shell startup

**Files:**
- Create: `tests/sigpipe_integration.rs`
- Modify: `src/shell.rs:519-526` (`install_job_control_signals`)

- [ ] **Step 1: Write the failing test**

Create `tests/sigpipe_integration.rs`:

```rust
//! v137: SIGPIPE is restored to SIG_DFL process-wide, so a producer writing to
//! a closed pipe dies silently (status 141) like bash instead of looping on
//! EPIPE and spamming "Broken pipe". Tests run the huck binary as a subprocess
//! (resetting SIGPIPE in the test process would not affect a spawned child).
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `huck -c <script>` with no stdin; return (stdout, stderr, exit_code).
fn huck_c(script: &str) -> (String, String, i32) {
    let o = Command::new(huck_bin())
        .arg("-c").arg(script)
        .stdin(Stdio::null())
        .output()
        .expect("spawn huck");
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

// A forked builtin producer whose consumer reads one line and exits must die on
// SIGPIPE: the producer stage status is 141 and NOTHING is printed to stderr.
#[test]
fn forked_producer_status_141_silent() {
    let (out, err, code) = huck_c(
        "{ for i in $(seq 1 5000); do echo $i; done; } | { read x; }; echo \"stages=${PIPESTATUS[*]}\"",
    );
    assert_eq!(code, 0, "overall rc; stderr={err:?}");
    assert_eq!(out, "stages=141 0\n", "producer must be SIGPIPE-killed (141); out={out:?}");
    assert_eq!(err, "", "no Broken pipe spam expected; err={err:?}");
}

// A 5000-line producer into `head -1` must emit exactly one line and ZERO
// "Broken pipe" lines on stderr (the assertion the fix fired).
#[test]
fn forked_producer_no_broken_pipe_spam() {
    let (out, err, _code) = huck_c(
        "for i in $(seq 1 5000); do echo \"line$i\"; done | head -1",
    );
    assert_eq!(out, "line1\n", "out={out:?}");
    assert!(!err.contains("Broken pipe"), "stderr leaked Broken pipe: {err:?}");
}
```

- [ ] **Step 2: Run the tests to verify they FAIL on current code**

Run: `cargo test --test sigpipe_integration 2>&1 | tail -25`
Expected: both FAIL — `forked_producer_status_141_silent` shows `out="stages=1 0\n"` (producer ran to completion, exit 1) and a non-empty `err` full of `huck: echo: Broken pipe (os error 32)`; `forked_producer_no_broken_pipe_spam` fails on the `Broken pipe` assertion.

- [ ] **Step 3: Apply the core fix in `src/shell.rs`**

In `install_job_control_signals()` (shell.rs:519), add the SIGPIPE reset after the existing loop:

```rust
fn install_job_control_signals() {
    for sig in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
        let prev = unsafe { libc::signal(sig, libc::SIG_IGN) };
        if prev == libc::SIG_ERR {
            eprintln!("huck: warning: could not ignore signal {sig}");
        }
    }
    // Rust's runtime sets SIGPIPE to SIG_IGN at startup; restore the OS default
    // so huck (and the stages it forks) die on a broken pipe like bash, instead
    // of getting EPIPE back from write(2) and looping. bash runs with SIGPIPE at
    // SIG_DFL everywhere; an interactive shell survives because its stdout is the
    // terminal, never a pipe. (v137)
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }
}
```

- [ ] **Step 4: Run the tests to verify they PASS**

Run: `cargo test --test sigpipe_integration 2>&1 | tail -25`
Expected: `forked_producer_status_141_silent` and `forked_producer_no_broken_pipe_spam` both PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/sigpipe_integration.rs src/shell.rs
git commit -m "$(printf 'fix: restore SIGPIPE default disposition at startup\n\nForked builtin/function producers now die silently on a broken pipe\n(status 141) like bash, instead of looping on EPIPE and spamming\n"Broken pipe".\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Explicit SIGPIPE→SIG_DFL in the forked-stage child + subshell producer test

**Files:**
- Modify: `src/executor.rs:~4727-4731` (`fork_and_run_in_subshell` child signal resets)
- Modify: `tests/sigpipe_integration.rs` (add subshell + function producer tests)

- [ ] **Step 1: Write the failing/strengthening tests**

Append to `tests/sigpipe_integration.rs`:

```rust
// A subshell `( ... )` producer is a forked stage; it too must die silently.
#[test]
fn subshell_producer_status_141_silent() {
    let (out, err, code) = huck_c(
        "( for i in $(seq 1 5000); do echo $i; done ) | { read x; }; echo \"stages=${PIPESTATUS[*]}\"",
    );
    assert_eq!(code, 0, "stderr={err:?}");
    assert_eq!(out, "stages=141 0\n", "out={out:?}");
    assert_eq!(err, "", "err={err:?}");
}

// A shell function producer (runs in the forked stage) must die silently too.
#[test]
fn function_producer_no_spam() {
    let (out, err, _c) = huck_c(
        "f(){ local i=0; while [ \"$i\" -lt 5000 ]; do echo \"$i\"; i=$((i+1)); done; }; f | head -2",
    );
    assert_eq!(out, "0\n1\n", "out={out:?}");
    assert!(!err.contains("Broken pipe"), "err={err:?}");
}
```

- [ ] **Step 2: Run them**

Run: `cargo test --test sigpipe_integration 2>&1 | tail -25`
Expected: PASS already (Change 1 makes the forked child inherit `SIG_DFL`). These tests lock in subshell + function coverage and guard Task 2's edit.

- [ ] **Step 3: Add the explicit reset in the forked-stage child**

In `src/executor.rs`, in the `fork_and_run_in_subshell` child block (the `if pid == 0 {` arm, currently resetting three job-control signals at ~executor.rs:4729), add the SIGPIPE line:

```rust
        unsafe {
            // 1. Reset job-control signals.
            libc::signal(libc::SIGTSTP, libc::SIG_DFL);
            libc::signal(libc::SIGTTIN, libc::SIG_DFL);
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            // v137: a forked pipeline stage / subshell dies on a broken pipe
            // like bash. Redundant with the startup reset in the common case,
            // but also correct for the PIPE-trap case: bash resets a trapped
            // signal to default inside a subshell, so a forked stage must not
            // inherit a top-level PIPE handler.
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            // 2. Join the pgrp (or become pgrp leader if pgid_target == 0).
            libc::setpgid(0, pgid_target);
```

(Insert the `SIGPIPE` line immediately after the three existing `SIG_DFL` resets and before the `setpgid` call. Keep the surrounding comments/lines intact.)

- [ ] **Step 4: Run the tests + clippy**

Run: `cargo test --test sigpipe_integration 2>&1 | tail -25`
Expected: all four PASS.
Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: no warnings introduced.

- [ ] **Step 5: Commit**

```bash
git add src/executor.rs tests/sigpipe_integration.rs
git commit -m "$(printf 'fix: explicit SIGPIPE SIG_DFL in forked pipeline stages/subshells\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Preserve the heredoc writer's manual EPIPE handling

**Files:**
- Modify: `src/executor.rs:~2608` (`spawn_heredoc_writer` child)

- [ ] **Step 1: Verify the heredoc tests currently pass (baseline)**

Run: `cargo test heredoc 2>&1 | tail -15`
Expected: existing heredoc tests PASS (record the count — they must stay green after Step 2).

- [ ] **Step 2: Restore SIG_IGN in the writer child**

In `src/executor.rs`, the `spawn_heredoc_writer` child block currently begins:

```rust
    if pid == 0 {
        // CHILD: async-signal-safe only. Close read end; write the body; _exit.
        unsafe { libc::close(r); }
        let mut off = 0usize;
```

Change the first `unsafe` block to also restore `SIG_IGN`, so the writer keeps its
existing manual `EPIPE` break instead of being killed by the new process-wide
`SIG_DFL`:

```rust
    if pid == 0 {
        // CHILD: async-signal-safe only. Close read end; write the body; _exit.
        // v137: keep SIGPIPE ignored here (the process is otherwise SIG_DFL now)
        // so the writer retains its manual EPIPE handling and closes cleanly,
        // preserving v134 large-heredoc behavior exactly.
        unsafe { libc::close(r); libc::signal(libc::SIGPIPE, libc::SIG_IGN); }
        let mut off = 0usize;
```

- [ ] **Step 3: Run the heredoc tests**

Run: `cargo test heredoc 2>&1 | tail -15`
Expected: same PASS count as Step 1 (no regression).

- [ ] **Step 4: Sanity-check a large heredoc with an early-closing consumer**

Build first if stale: `cargo build 2>&1 | tail -2`. Then run a ~100k-line heredoc
body piped into `head -1` (the consumer closes after one line, so the forked
writer hits EPIPE and must break cleanly, not spam or hang):

Run: `target/debug/huck -c "$(printf 'cat <<EOF | head -1\n'; seq 1 100000; printf 'EOF\n')" 2>&1 | tail -3`
Expected: prints `1`, no `Broken pipe` output, completes promptly.

- [ ] **Step 5: Commit**

```bash
git add src/executor.rs
git commit -m "$(printf 'fix: keep SIGPIPE ignored in the heredoc writer child (preserve v134)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Main-process termination test + `trap … PIPE` settable test

**Files:**
- Modify: `tests/sigpipe_integration.rs` (add two tests)

- [ ] **Step 1: Write the tests**

Append to `tests/sigpipe_integration.rs` (add the imports shown at the top of the snippet to the existing `use` lines if not present):

```rust
use std::io::Read;
use std::time::{Duration, Instant};

// A producer in huck's MAIN process (huck's own stdout is the pipe) must die on
// SIGPIPE when the reader closes — terminate (rc 141), no infinite loop, no spam.
#[test]
fn main_process_producer_terminates_on_broken_pipe() {
    let mut child = Command::new(huck_bin())
        .arg("-c").arg("while true; do printf 'x\\n'; done")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");

    // Read one byte, then close the read end so huck's next write gets SIGPIPE.
    {
        let mut so = child.stdout.take().unwrap();
        let mut one = [0u8; 1];
        so.read_exact(&mut one).expect("read first byte");
        assert_eq!(&one, b"x");
        // `so` dropped here -> read end closed.
    }

    // Watchdog: huck must exit on its own within a few seconds.
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(st) = child.try_wait().expect("try_wait") { break st; }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("huck did not terminate on a broken pipe (infinite loop)");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(141), "expected SIGPIPE exit 141; got {status:?}");

    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).ok();
    assert!(!err.contains("Broken pipe"), "stderr leaked Broken pipe: {err:?}");
}

// Restoring SIG_DFL at startup makes SIGPIPE trappable again (was rejected with
// "cannot reset ignored signal").
#[test]
fn trap_pipe_is_now_settable() {
    let (out, err, code) = huck_c("trap 'echo handler' PIPE; echo set-ok");
    assert_eq!(out, "set-ok\n", "out={out:?}");
    assert_eq!(code, 0, "code={code}");
    assert!(!err.contains("cannot reset ignored signal"), "err={err:?}");
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --test sigpipe_integration 2>&1 | tail -25`
Expected: all six PASS. (`main_process_producer_terminates_on_broken_pipe` would have hung pre-fix; `trap_pipe_is_now_settable` would have shown the rejection pre-fix.)

- [ ] **Step 3: Commit**

```bash
git add tests/sigpipe_integration.rs
git commit -m "$(printf 'test: main-process SIGPIPE termination + trap PIPE settable\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Bash-diff harness (the 57th)

**Files:**
- Create: `tests/scripts/sigpipe_diff_check.sh`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/sigpipe_diff_check.sh` (capture stdout and stderr SEPARATELY — a SIGPIPE-killed producer can interleave with consumer output under a combined `2>&1`):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v137: a producer writing to a closed
# pipe dies silently (SIGPIPE, status 141) instead of spamming "Broken pipe".
# stdout AND stderr are captured separately and both must match bash.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2"
    local bo be ho he
    bo=$(bash -c "$frag" 2>/tmp/v137_be); be=$(cat /tmp/v137_be)
    ho=$("$HUCK_BIN" -c "$frag" 2>/tmp/v137_he); he=$(cat /tmp/v137_he)
    if [[ "$bo" == "$ho" && "$be" == "$he" ]]; then
        printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        [[ "$bo" != "$ho" ]] && { echo "  stdout diff:"; diff <(echo "$bo") <(echo "$ho") | sed 's/^/    /'; }
        [[ "$be" != "$he" ]] && { echo "  stderr diff:"; diff <(echo "$be") <(echo "$he") | sed 's/^/    /'; }
        FAIL=$((FAIL+1))
    fi
}
check "printf producer | head"   '{ for i in $(seq 1 5000); do printf "%d\n" "$i"; done; } | head -3'
check "echo producer | head"     '{ for i in $(seq 1 5000); do echo "$i"; done; } | head -3'
check "function producer | head" 'f(){ local i=0; while [ "$i" -lt 5000 ]; do echo "$i"; i=$((i+1)); done; }; f | head -2'
check "subshell producer | head" '( for i in $(seq 1 5000); do echo "$i"; done ) | head -2'
check "external producer | read" 'seq 1 5000 | { read x; echo "first=$x"; }'
check "trap ignore PIPE"         'trap "" PIPE; echo set-ok'
check "trap handler PIPE"        'trap "echo h" PIPE; echo set-ok'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
rm -f /tmp/v137_be /tmp/v137_he
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make it executable and build the debug binary**

Run: `chmod +x tests/scripts/sigpipe_diff_check.sh && cargo build 2>&1 | tail -3`
Expected: build succeeds.

- [ ] **Step 3: Run the harness**

Run: `bash tests/scripts/sigpipe_diff_check.sh`
Expected: `Total: 7, Pass: 7, Fail: 0`.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/sigpipe_diff_check.sh
git commit -m "$(printf 'test: 57th bash-diff harness for SIGPIPE default disposition\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 6: Docs — reopen-and-delete Tier-1, note trap PIPE

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Add a Tier-1 entry for the bug being fixed, then immediately delete it on this same branch**

The doc is a CURRENT-divergences-only reference. Because v137 fixes the bug, the
net result is NO Tier-1 entry. Concretely:
- Confirm the Summary table "Bugs (Tier 1)" count stays `0` (the fix lands in the
  same iteration, so we do not leave a standing Tier-1 entry).
- Under Tier 2 "Job control" or the existing M-22 area, update the note that
  `trap … PIPE` is now accepted. Find the M-22 reference (the "Out of scope"
  note about `trap '' PIPE` not propagating SIG_IGN across exec) and append:

```
  NOTE (v137): `trap … PIPE` is now SETTABLE (SIGPIPE is restored to SIG_DFL at
  startup, so it is no longer in the ignored-at-startup set). A top-level PIPE
  trap fires via the flag-based dispatch; a forked pipeline stage resets PIPE to
  SIG_DFL (matching bash's subshell-resets-traps), so a trap does not fire inside
  a pipeline subshell. The remaining gap is only the `trap '' PIPE` ignore-form
  not staying ignored inside a subshell (huck represents it as a handler, not a
  real SIG_IGN) — unchanged, low impact.
```

  (If the M-22 text lives under a different ID after prior slimming, attach the
  note to whichever entry documents the PIPE-trap / SIGPIPE propagation
  limitation. If no such entry exists, add a brief `[low]`/`[intentional]` Tier-4
  note titled "trap '' PIPE ignore-form not preserved in a subshell".)

- [ ] **Step 2: Verify the Summary counts are consistent**

Read the Summary table at the top of `docs/bash-divergences.md`. Confirm:
- Bugs (Tier 1) = `0` (unchanged — v137 is a same-iteration fix, no standing entry).
- If a Tier-4 note was added in Step 1, bump the Low-impact count by 1; otherwise leave counts unchanged.

- [ ] **Step 3: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "$(printf 'docs: note trap PIPE now settable after v137 SIGPIPE fix\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 7: Full regression

**Files:** none (verification only)

- [ ] **Step 1: Run the entire test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: ALL tests pass (the prior baseline was 3009 tests after v136; the new
`sigpipe_integration` adds 6). Zero failures.

- [ ] **Step 2: Run the PTY / job-control suites explicitly (the paths v137 touches)**

Run: `cargo test --test pty_interactive --test subshell_pipeline_pty --test completion_jobcontrol_pty --test subshell_tty_pty 2>&1 | tail -20`
Expected: pass (or skip gracefully if no PTY is available, matching their existing behavior). Ctrl-Z stop, subshell tty hand-off, and completion job-control must be unaffected by the SIGPIPE change.

- [ ] **Step 3: Run ALL bash-diff harnesses**

Run: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do echo "== $f =="; bash "$f" | tail -1; done`
Expected: every harness ends with `Fail: 0` (including the new `sigpipe_diff_check.sh` → `Pass: 7, Fail: 0`).

- [ ] **Step 4: Clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: clean (no new warnings).

- [ ] **Step 5: End-to-end payoff check (manual, optional but recommended)**

Confirm the original symptom is gone using the real driver (do NOT source
`~/.bashrc` — it holds credentials; use `~/.nvm/nvm.sh` directly):

Run: `target/release/huck -c 'source ~/.nvm/nvm.sh; nvm ls' 2>&1 | tail -5` (after `cargo build --release`), then in an interactive session verify Ctrl-C during `nvm ls` returns cleanly with no `Broken pipe` spam.
Expected: no `huck: printf: Broken pipe` lines; output matches bash.

- [ ] **Step 6: Commit (only if any verification-driven fix was needed)**

If Steps 1–4 surfaced a real issue, fix it (smallest change), re-run, and commit
with the standard trailer. Otherwise no commit — this task is verification.

---

## Notes for the implementer

- **Do not modify `builtins.rs`.** With `SIGPIPE = SIG_DFL`, the `echo`/`printf`
  EPIPE branches are unreachable in normal use; they correctly handle genuine
  non-EPIPE write errors and the rare `trap '' PIPE` context. No suppress helper.
- **Do not reset SIGPIPE for external children** (`reset_job_control_signals_in_child`):
  `std::process::Command` already resets it before exec.
- **Status 141** comes for free from the existing `128 + signum` mapping in
  `wait_pipeline_raw` (executor.rs:~2730) — no status-handling code is needed.
- **If a PTY suite cannot run** in the environment, confirm it SKIPS (its existing
  no-PTY behavior) rather than fails; do not weaken assertions to force a pass.
