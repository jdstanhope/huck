# v285 — Fix 100ms foreground-child wait latency — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the ~100ms latency every foreground external command and
subshell pays when its stdio is inherited (not captured), per issue
[#120](https://github.com/jdstanhope/huck/issues/120).

**Architecture:** `stream_loop::external_capture_loop` is the foreground-child
wait. When there are no capture pipes to stream and no embedder deadline, it
currently falls back to a `poll(0 fds) + sleep(100ms)` tick. Replace that case
with a plain blocking `waitpid`. The capture/streaming path is untouched.

**Tech Stack:** Rust, `libc` (`waitpid`), existing `tests/scripts/*_diff_check.sh`
harness convention, `huck-engine` integration test binaries via the public
`Engine` API.

## Global Constraints

- Commit trailer on every commit:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- `cargo fmt --all` before every commit; CI enforces `--check`.
- Run engine tests per-crate, single-threaded (the dev box OOMs on
  `--workspace`): `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
  Build the binary with `cargo build -p huck`.
- The fix must be behavior-preserving except for latency: no `WUNTRACED` (match
  the loop it replaces), re-wait on `EINTR`, reap the pid (caller
  `std::mem::forget`s the `Child`).
- Do NOT touch the capture/streaming path (any run with a pipe present) or
  pipelines.

---

### Task 1: Blocking fast-path in `external_capture_loop`

**Files:**
- Modify: `crates/huck-engine/src/stream_loop.rs`
- Test: `crates/huck-engine/src/stream_loop.rs` (new `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing new.
- Produces: `fn blocking_wait(child_pid: libc::pid_t) -> io::Result<i32>`
  (private); a new early-return branch in the existing
  `pub fn external_capture_loop(child_pid, pipe_out, pipe_err, sinks, mut timeout_remaining) -> io::Result<i32>`.

- [ ] **Step 1: Write the failing test**

Add at the end of `crates/huck-engine/src/stream_loop.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn no_pipe_wait_is_prompt_and_correct() {
        // Fork a child that exits(7) immediately. The no-pipe / no-timeout
        // fast path must return its status without the old ~100ms poll-tick
        // latency (regression guard for #120).
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe { libc::_exit(7) };
        }
        let sinks = CaptureSinks {
            stdout: None,
            stderr: None,
        };
        let start = Instant::now();
        let status = external_capture_loop(pid, -1, -1, sinks, || None).unwrap();
        let elapsed = start.elapsed();
        assert!(libc::WIFEXITED(status), "child did not exit normally");
        assert_eq!(libc::WEXITSTATUS(status), 7, "wrong exit status");
        assert!(
            elapsed < Duration::from_millis(50),
            "no-pipe wait took {elapsed:?}; expected prompt return (#120 regression)"
        );
    }
}
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 stream_loop::tests::no_pipe_wait_is_prompt_and_correct`
Expected: FAIL — the current no-pipe path sleeps ~100ms per tick, so `elapsed`
exceeds 50ms (the exit status assertions pass; the timing one fails).

- [ ] **Step 3: Add the fast path and helper**

In `crates/huck-engine/src/stream_loop.rs`, at the very top of
`external_capture_loop` (immediately after the `pub fn ... -> io::Result<i32> {`
line, before `let mut wl = WaitLoop::new()?;`), insert:

```rust
    // #120: With no capture pipes to stream AND no embedder deadline, there is
    // nothing for the poll loop to watch — it would fall back to sleeping
    // POLL_TICK_MS (100ms) before each reap, so every foreground external
    // command / subshell with inherited stdio paid ~100ms. Block on the child
    // directly instead. Behavior-equivalent for signals/traps (both re-wait on
    // EINTR); no pipes means no final drain is needed.
    if pipe_out < 0 && pipe_err < 0 && timeout_remaining().is_none() {
        return blocking_wait(child_pid);
    }
```

Then add this free function directly above `pub fn external_capture_loop`:

```rust
/// Block until `child_pid` exits, retrying on `EINTR` so a signal delivered to
/// the shell (e.g. a trap) is handled and the wait resumes. Returns the raw
/// `waitpid` status. Used by `external_capture_loop`'s no-pipe / no-timeout
/// fast path, where there is nothing to stream. Flags `0` (no `WUNTRACED`)
/// match the poll loop it replaces; foreground job-control stop handling lives
/// on the interactive path, not here.
fn blocking_wait(child_pid: libc::pid_t) -> io::Result<i32> {
    loop {
        let mut status: i32 = 0;
        let r = unsafe { libc::waitpid(child_pid, &mut status, 0) };
        if r == child_pid {
            return Ok(status);
        }
        if r < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(err);
        }
        // r == 0 is impossible without WNOHANG; loop defensively.
    }
}
```

- [ ] **Step 4: Run the test to confirm it passes**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 stream_loop`
Expected: PASS (both the new test and the existing `wait_loop`/stream tests).

- [ ] **Step 5: Format and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/stream_loop.rs
git commit -m "fix(#120): block on the child when there are no capture pipes

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Byte-identical correctness harness

**Files:**
- Create: `tests/scripts/external_wait_latency_diff_check.sh` (mode 0755)

**Interfaces:**
- Consumes: the debug binary at `target/debug/huck` (built with
  `cargo build -p huck`).
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Build the binary**

Run: `cargo build -p huck`
Expected: builds `target/debug/huck`.

- [ ] **Step 2: Write the harness**

Create `tests/scripts/external_wait_latency_diff_check.sh` with exactly:

```bash
#!/usr/bin/env bash
# v285 (#120): the no-pipe foreground-wait fast path must not change any
# observable behavior — output across many external commands and subshells
# (inherited, redirected, and captured stdio) stays byte-identical to bash.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Subshells with inherited stdio (no capture pipe — the fixed path).
check "subshell loop inherited"   'for i in 1 2 3 4 5; do ( echo "s$i" ); done'
check "empty subshell loop"       'for i in 1 2 3; do ( : ); done; echo done'
check "subshell exit status"      '( exit 3 ); echo "rc=$?"'
check "nested subshell"           '( ( ( echo deep ) ) ); echo "rc=$?"'
# External commands with inherited stdio.
check "external loop inherited"   'for i in 1 2 3 4 5; do /bin/echo "e$i"; done'
check "external exit status"      '/bin/false; echo "rc=$?"'
check "external then builtin"     '/bin/true && echo ok'
# Redirected (still no capture pipe on the shell side).
check "subshell redirected"       '( echo hidden ) >/dev/null; echo shown'
check "external redirected"       '/bin/echo hidden >/dev/null; echo shown'
# Captured path must be unchanged too.
check "command substitution"      'x=$( ( echo cap ) ); echo "[$x]"'
check "external in capture"       'x=$(/bin/echo capext); echo "[$x]"'
# Mixed sequence.
check "mixed sequence"            '( echo a ); /bin/echo b; echo c; x=$(echo d); echo "$x"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 3: Make it executable and run it**

```bash
chmod 0755 tests/scripts/external_wait_latency_diff_check.sh
bash tests/scripts/external_wait_latency_diff_check.sh
```
Expected: every check `PASS`, final line `Fail: 0`, exit 0.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/external_wait_latency_diff_check.sh
git commit -m "test(#120): byte-identical harness for the no-pipe wait fast path

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Coarse wall-clock timing integration test

**Files:**
- Create: `crates/huck-engine/tests/foreground_wait_latency.rs`

**Interfaces:**
- Consumes: the public `huck_engine::Engine` API — `Engine::new()` and
  `engine.run(src: &str) -> i32` (uses `StdoutSink::Terminal`, i.e. inherited
  stdout, so subshells/externals inside take the no-pipe path).
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Write the timing test**

Create `crates/huck-engine/tests/foreground_wait_latency.rs` with exactly:

```rust
//! Coarse wall-clock guard for #120: with inherited stdout (Engine::run, not
//! capture), running many subshells / external commands must NOT pay the old
//! ~100ms-per-child poll-tick latency. Pre-fix each 50-child batch took ~5s
//! (50 x 100ms); post-fix it is well under 0.5s. The 3s ceiling is generous
//! enough to be robust on a loaded 1-core box while still failing loudly if
//! this exact latency regresses. Its own integration binary so it never shares
//! a process with other forking tests.

use std::time::{Duration, Instant};

use huck_engine::Engine;

const CEILING: Duration = Duration::from_secs(3);

#[test]
fn fifty_subshells_are_prompt() {
    let mut e = Engine::new();
    let start = Instant::now();
    // 50 empty subshells with inherited stdio (no capture pipe).
    let code = e.run("for i in $(seq 50); do ( : ); done");
    let elapsed = start.elapsed();
    assert_eq!(code, 0, "script exit code");
    assert!(
        elapsed < CEILING,
        "50 subshells took {elapsed:?} (>= {CEILING:?}); the #120 100ms-per-child latency has regressed"
    );
}

#[test]
fn fifty_external_commands_are_prompt() {
    let mut e = Engine::new();
    let start = Instant::now();
    // 50 external commands with output redirected away (still no shell-side
    // capture pipe).
    let code = e.run("for i in $(seq 50); do /bin/true; done");
    let elapsed = start.elapsed();
    assert_eq!(code, 0, "script exit code");
    assert!(
        elapsed < CEILING,
        "50 external commands took {elapsed:?} (>= {CEILING:?}); the #120 latency has regressed"
    );
}
```

- [ ] **Step 2: Run it (must fail before Task 1 is present; here it should pass)**

Run: `cargo test -p huck-engine --test foreground_wait_latency --jobs 1 -- --test-threads 1`
Expected: PASS (Task 1 is already committed). To confirm the guard has teeth,
optionally `git stash` Task 1, rerun (each batch ~5s > 3s → FAIL), then
`git stash pop`.

- [ ] **Step 3: Commit**

```bash
git add crates/huck-engine/tests/foreground_wait_latency.rs
git commit -m "test(#120): coarse wall-clock guard for foreground-child wait latency

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — green
  (includes `stream_loop::tests`).
- [ ] `cargo test -p huck-engine --test foreground_wait_latency --jobs 1 -- --test-threads 1` — green.
- [ ] `cargo build -p huck` then
  `bash tests/scripts/external_wait_latency_diff_check.sh` — `Fail: 0`.
- [ ] Build release + run the diff sweep (`cargo build --release --locked --bin
  huck` then `tests/scripts/run_diff_checks.sh`) — green.
- [ ] Sanity: `time target/release/huck -c '( : )'` and `time target/release/huck
  -c '/bin/true'` are now single-digit ms (were ~104ms / ~109ms).

## Self-review

- Spec coverage: fix (Task 1), correctness guard (Task 2), timing guard
  (Task 3) — all three spec testing items covered.
- Type consistency: `blocking_wait(libc::pid_t) -> io::Result<i32>` used in
  Task 1; `Engine::new()` / `run(&str) -> i32` used in Task 3 (verified against
  `engine.rs`); `CaptureSinks { stdout, stderr }` fields match `stream_loop.rs`.
- No placeholders.
