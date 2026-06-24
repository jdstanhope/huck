# v212 Pipeline non-final stage redirect EOF (M-125) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix M-125: a non-final pipeline stage with an explicit stdout redirect (`>file` / `>>file`) leaks the parent's stdin into the downstream stage instead of creating an EOF-on-read pipe. Causes huck to HANG where bash returns when parent stdin is blocking (terminal / FIFO).

**Architecture:** Add a small `make_orphan_pipe_for_eof_reader` helper next to `make_pipe` in `executor.rs`: creates a pipe, closes the write-end immediately, returns the read-end. Two pipeline-builder sites (`run_pipeline` line 3085 and `run_multi_stage` line 5800) get an updated `stdout_fd` selection that ALSO calls the helper in the `Some(fd)` + `!is_last` case (upstream still goes to the file; downstream gets the orphan pipe's read-end as its stdin → EOF).

**Tech Stack:** Rust 2024, no new deps. Plain `libc::pipe` / `libc::close`. The new harness uses bash + huck, same shape as `tests/scripts/array_transforms_diff_check.sh`.

**Branch:** `v212-pipeline-redirect-pipe`. Each task ends with a green-suite commit.

**Spec:** `docs/superpowers/specs/2026-06-24-pipeline-redirect-pipe-design.md`.

**Key context — current code shapes** (verified pre-plan):

- `crates/huck-engine/src/executor.rs:5360-5366` — `fn make_pipe() -> io::Result<(RawFd, RawFd)>`. Plain wrapper around `libc::pipe`.
- `crates/huck-engine/src/executor.rs:2658` — `fn run_pipeline(...)`. The interactive / job-control path.
- `crates/huck-engine/src/executor.rs:3085-3108` — `run_pipeline`'s stdout-fd selection. Bug site #1. The `make_pipe` Err arm here calls `cleanup_partial_pipeline_raw(first_pid, &spawned_pids)` (pipeline-specific) before returning.
- `crates/huck-engine/src/executor.rs:5390` — `fn run_multi_stage(...)`. The multi-stage helper.
- `crates/huck-engine/src/executor.rs:5800-5821` — `run_multi_stage`'s stdout-fd selection. Bug site #2. The `make_pipe` Err arm here drains `parent_held` but does NOT call any pipeline cleanup function (different in-flight stage tracking — `live_pids_arc`).
- Both Err arms share the same general shape: print "huck: pipe: {e}" → restore inline assignments → close stdin_fd if > 2 → close explicit_stderr_fd if set → drain procsubs → drain parent_held → return `ExecOutcome::Continue(1)`.

---

## File structure

**Modify:**
- `crates/huck-engine/src/executor.rs` — add `make_orphan_pipe_for_eof_reader` helper; update both stdout-fd selection arms (lines 3085 and 5800).
- `docs/bash-divergences.md` — delete M-125 entry; update Tier 2 count 11 → 10.

**Create:**
- `tests/scripts/pipeline_redirect_pipe_diff_check.sh` — 7-fragment bash-diff harness.

No new modules. No public API change. No architecture impact.

---

## Task 1: Add `make_orphan_pipe_for_eof_reader` helper + unit test

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — add helper next to `make_pipe`; add unit test in `mod tests`.

- [ ] **Step 1: Create the branch**

```bash
git checkout main
git pull --ff-only
git checkout -b v212-pipeline-redirect-pipe
```

- [ ] **Step 2: Add the helper**

Find `make_pipe`:

```bash
grep -n "fn make_pipe" crates/huck-engine/src/executor.rs
```

Should be around line 5360. Add the new helper IMMEDIATELY AFTER `make_pipe` (so the two pipe primitives live together):

```rust
/// Create an inter-stage pipe for a downstream pipeline reader, where
/// the upstream stage's stdout is going elsewhere (an explicit file
/// redirect). Closes the write-end immediately so the downstream reader
/// sees EOF instead of inheriting parent stdin or blocking on an
/// orphaned write-end. Returns the read-end fd to thread into
/// `prev_pipe_read`. On `make_pipe` failure, the caller propagates the
/// error.
fn make_orphan_pipe_for_eof_reader() -> io::Result<RawFd> {
    let (r, w) = make_pipe()?;
    unsafe { libc::close(w); }
    Ok(r)
}
```

The `RawFd` and `io` paths are already in scope at the top of the file via the existing `make_pipe` definition.

- [ ] **Step 3: Add the unit test**

Find the existing `#[cfg(test)] mod tests` block in `executor.rs`:

```bash
grep -n "#\[cfg(test)\]" crates/huck-engine/src/executor.rs | head -3
```

Append:

```rust
    #[test]
    fn make_orphan_pipe_for_eof_reader_yields_immediate_eof() {
        use std::io::Read;
        use std::os::unix::io::FromRawFd;
        let r = super::make_orphan_pipe_for_eof_reader().expect("pipe");
        // Read should return 0 bytes (EOF) immediately, not block.
        let mut f = unsafe { std::fs::File::from_raw_fd(r) };
        let mut buf = [0u8; 8];
        let n = f.read(&mut buf).expect("read");
        assert_eq!(n, 0, "expected EOF, got {n} bytes");
    }
```

The `super::` prefix is needed because `make_pipe` and the new helper are file-private (not pub). The unit test lives in the same `mod tests` inside the file, so it CAN reach private items via `super::`.

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet make_orphan_pipe_for_eof_reader_yields_immediate_eof
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 1 new test passes; full suite green; clippy clean.

If clippy complains about the helper being unused (which it shouldn't because the test calls it), add a `#[allow(dead_code)]` temporarily — it'll be removed in Task 2 when the production callers land.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v212 task 1: make_orphan_pipe_for_eof_reader helper

New helper next to make_pipe that creates a pipe, closes the write-end
immediately, and returns the read-end. Lets a downstream pipeline stage
read EOF when the upstream stage's stdout is going to a file rather
than the pipe. One unit test asserts the returned fd reads 0 bytes
immediately (the write-end really is closed before the helper returns).
No production callers yet — Tasks 2 and 3 wire it into run_pipeline
and run_multi_stage.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Wire helper into `run_pipeline` (executor.rs:3085)

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — update the `let stdout_fd: RawFd = ...` block at line 3085.

- [ ] **Step 1: Locate the buggy block**

```bash
grep -n "let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd" crates/huck-engine/src/executor.rs
```

Should hit two lines: 3085 (this task) and 5800 (Task 3). The current `run_pipeline` block at line 3085-3108 reads:

```rust
        let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd {
            fd
        } else if !is_last {
            match make_pipe() {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    parent_held.push(w);
                    w
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                    restore_inline_assignments(snap, shell);
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                    drain_procsubs(shell, procsub_base);
                    cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                    for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            libc::STDOUT_FILENO
        };
```

- [ ] **Step 2: Replace with the fixed shape**

```rust
        let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd {
            // Upstream stdout goes to the file. For a non-final stage we
            // STILL need to create an inter-stage pipe so the downstream
            // stage reads EOF instead of inheriting parent stdin (M-125).
            if !is_last {
                match make_orphan_pipe_for_eof_reader() {
                    Ok(r) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                        restore_inline_assignments(snap, shell);
                        if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                        if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                        unsafe { libc::close(fd); } // close the open file fd we won't use
                        drain_procsubs(shell, procsub_base);
                        cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                        for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            fd
        } else if !is_last {
            match make_pipe() {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    parent_held.push(w);
                    w
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                    restore_inline_assignments(snap, shell);
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                    drain_procsubs(shell, procsub_base);
                    cleanup_partial_pipeline_raw(first_pid, &spawned_pids);
                    for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            libc::STDOUT_FILENO
        };
```

The cleanup arm in the new helper-Err branch MIRRORS the existing `make_pipe()` Err arm in this function — same restore_inline_assignments / stdin close / explicit_stderr_fd close / drain procsubs / cleanup_partial_pipeline_raw / parent_held drain. The ONLY addition is `unsafe { libc::close(fd); }` (the open file fd — since we're returning Err before using it, we need to close it manually).

NOTE: don't reference v212 / "M-125" / task numbers in source comments. The "(M-125)" parenthetical in the comment is fine because M-125 is a divergence ID (like L-44), not an iteration version.

- [ ] **Step 3: Build + smoke test**

```bash
cargo build --workspace -q
cargo build --release --workspace --quiet
printf 'hello\n' | ./target/release/huck -c 'echo upstream > /tmp/m125-out | cat'
echo "exit=$?"
```

Expected: empty stdout (no "hello"), `exit=0`. Compare to:

```bash
printf 'hello\n' | bash -c 'echo upstream > /tmp/m125-out | cat'
echo "exit=$?"
```

Should also be empty stdout, `exit=0` — byte-identical to the new huck behavior. The bug is now fixed in `run_pipeline`.

But the SAME bug ALSO exists at line 5800 (`run_multi_stage`). The CLI's `huck -c` path goes through `run_program_in_sinks` → eventually one of these two functions. Whether the smoke above exercises `run_pipeline` vs `run_multi_stage` depends on the dispatch. Both need fixing — Task 3 handles the other site.

- [ ] **Step 4: Run tests + clippy**

```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green; clippy clean. Some existing pipeline-related tests may light up if they exercise this code path — investigate any failure; should be zero.

If clippy fires `#[warn(dead_code)]` on the helper now, it's been removed by the production caller in Step 2 — good.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v212 task 2: wire orphan-pipe helper into run_pipeline (M-125 site 1)

In run_pipeline's stdout-fd selection, when explicit_stdout_fd is set
AND we're not on the final stage, also create the orphan inter-stage
pipe so the downstream stage's stdin reads EOF (matches bash). The
upstream stage still gets the open file fd as its stdout; the pipe's
write-end is already closed by the helper. The new Err-arm mirrors the
existing make_pipe()-Err arm in the same function, with one addition:
close the open file fd we no longer use.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Wire helper into `run_multi_stage` (executor.rs:5800)

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — update the `let stdout_fd: RawFd = ...` block at line 5800.

- [ ] **Step 1: Locate the second buggy block**

```bash
grep -n "let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd" crates/huck-engine/src/executor.rs
```

Should still hit line 5800 (Task 2 already fixed line 3085). The current `run_multi_stage` block at 5800-5821 reads (NOTE: differs from `run_pipeline`'s cleanup — uses parent_held drain only, NO `cleanup_partial_pipeline_raw` call):

```rust
        let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd {
            fd
        } else if !is_last {
            // Create the inter-stage pipe.
            match make_pipe() {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    // w is given to the child; track it so other children can close it.
                    parent_held.push(w);
                    w
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                    restore_inline_assignments(snap, shell);
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                    drain_procsubs(shell, procsub_base);
                    for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            /* … the existing Capture/Terminal branch unchanged … */
        };
```

- [ ] **Step 2: Replace with the fixed shape**

```rust
        let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd {
            // Upstream stdout goes to the file. For a non-final stage we
            // STILL need to create an inter-stage pipe so the downstream
            // stage reads EOF instead of inheriting parent stdin (M-125).
            if !is_last {
                match make_orphan_pipe_for_eof_reader() {
                    Ok(r) => {
                        prev_pipe_read = Some(r);
                        parent_held.push(r);
                    }
                    Err(e) => {
                        { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                        restore_inline_assignments(snap, shell);
                        if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                        if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                        unsafe { libc::close(fd); } // close the open file fd we won't use
                        drain_procsubs(shell, procsub_base);
                        for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            fd
        } else if !is_last {
            // Create the inter-stage pipe.
            match make_pipe() {
                Ok((r, w)) => {
                    prev_pipe_read = Some(r);
                    parent_held.push(r);
                    // w is given to the child; track it so other children can close it.
                    parent_held.push(w);
                    w
                }
                Err(e) => {
                    { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: pipe: {e}"); }
                    restore_inline_assignments(snap, shell);
                    if stdin_fd > 2 { unsafe { libc::close(stdin_fd); } }
                    if let Some(fd) = explicit_stderr_fd { unsafe { libc::close(fd); } }
                    drain_procsubs(shell, procsub_base);
                    for fd in parent_held.drain(..) { unsafe { libc::close(fd); } }
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            /* … leave the existing Capture/Terminal else-branch UNCHANGED … */
        };
```

The "(M-125)" parenthetical is fine in source — divergence ID, not an iteration version.

When pasting, KEEP the existing `else { match sink { StdoutSink::Capture(...) ... } StdoutSink::Terminal => ... } }` branch verbatim; only the `if let Some(fd) ...` branch changes. Easiest: use Edit to replace JUST the `if let Some(fd) = explicit_stdout_fd { fd }` arm with the new arm, leaving the rest of the chain alone.

NOTE: the cleanup arm here differs from Task 2's `run_pipeline` cleanup — no `cleanup_partial_pipeline_raw(first_pid, &spawned_pids)` call. Match the EXISTING `make_pipe()` Err arm in `run_multi_stage` exactly.

- [ ] **Step 3: Build + smoke test**

```bash
cargo build --workspace -q
cargo build --release --workspace --quiet
printf 'hello\n' | ./target/release/huck -c 'echo upstream > /tmp/m125-out | cat'
echo "exit=$?"
```

Expected: empty stdout, exit=0. Both pipeline sites are now fixed.

Additional smoke for the stderr-redirect no-bug guard:

```bash
printf 'X' | ./target/release/huck -c 'echo up 2>/tmp/m125-err | cat'
echo "exit=$?"
```

Expected: "up\n" (echo writes to stdout → through inter-stage pipe → cat prints), exit=0.

- [ ] **Step 4: Run tests + clippy**

```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: green; clippy clean.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v212 task 3: wire orphan-pipe helper into run_multi_stage (M-125 site 2)

Mirror of Task 2 but at run_multi_stage's stdout-fd selection (line
5800). Same Err-arm cleanup as the existing make_pipe()-Err arm in the
same function (no cleanup_partial_pipeline_raw — run_multi_stage uses
parent_held drain + live_pids_arc bookkeeping, distinct from
run_pipeline's spawned_pids). With both call sites updated, the
${cmd >file | next} hang is closed across every pipeline entry point.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Bash-diff harness `pipeline_redirect_pipe_diff_check.sh`

**Files:**
- Create: `tests/scripts/pipeline_redirect_pipe_diff_check.sh`

- [ ] **Step 1: Create the harness**

```bash
cat > tests/scripts/pipeline_redirect_pipe_diff_check.sh <<'HARNESS_EOF'
#!/usr/bin/env bash
# v212: bash-diff harness for the M-125 fix. A non-final pipeline stage
# with explicit stdout redirect must give the downstream stage an EOF
# inter-stage pipe (not the parent's stdin).
set -u

cd "$(dirname "$0")/../.." || exit 1
cargo build --quiet --workspace --bin huck >/dev/null 2>&1
HUCK=target/debug/huck
if [ ! -x "$HUCK" ]; then
    echo "FAIL: huck binary not found at $HUCK" >&2
    exit 1
fi

FAIL=0
check() {
    local label=$1 frag=$2
    local b h
    # Each fragment carries its own `printf '...' |` stdin feed inside
    # the fragment string. We pass the fragment to `bash -c` / `huck -c`
    # with the same parent stdin (the harness's own stdin), so the
    # fragment's piped stdin is what each shell actually sees.
    b=$(bash -c "$frag" 2>&1)
    h=$("$HUCK" -c "$frag" 2>&1)
    if [ "$b" != "$h" ]; then
        echo "FAIL [$label]"
        echo "  bash: $b"
        echo "  huck: $h"
        FAIL=1
    else
        echo "PASS [$label]"
    fi
}

# Use a stable temp filename per run.
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT

# === The bug fix: non-final stdout redirect now produces EOF for next stage ===
check 'stdout-trunc-non-final-eof'  "printf 'X' | { echo up > $T/o | cat; }"
check 'stdout-append-non-final-eof' "printf 'X' | { echo up >> $T/o | cat; }"
check 'stdout-redir-3-stage'        "printf 'X' | { echo a > $T/o | echo b | cat; }"

# === No-bug regression guards: these paths already worked, must keep working ===
check 'stderr-only-redir-no-bug'    "printf 'X' | { echo up 2> $T/e | cat; }"
# >&2 routes upstream stdout to stderr; both shells should produce no stdout
# (the "up" goes to stderr, which we drop). The downstream stage gets the
# inter-stage pipe (already created in the normal path) and reads EOF.
check 'dup-redirect-no-bug'         "printf 'X' | { echo up >&2 | cat; } 2>/dev/null"
check 'final-stage-redir-no-bug'    "printf 'X' | { cat | tee $T/o >/dev/null; cat $T/o; }"

# === Redirect-open failure: error path, no fd leak ===
# bash and huck use different exact error wording; normalize by extracting
# just the "No such file" portion which is libc-uniform.
check 'redir-failure-still-skips'   "printf 'X' | { echo up >/no/such/dir/m125 | cat; } 2>&1 | grep -oE 'No such file or directory' | head -1"

if [ $FAIL -ne 0 ]; then
    echo "pipeline_redirect_pipe_diff_check FAILED" >&2
    exit 1
fi
echo "pipeline_redirect_pipe_diff_check OK"
HARNESS_EOF
chmod +x tests/scripts/pipeline_redirect_pipe_diff_check.sh
```

- [ ] **Step 2: Run the harness**

```bash
bash tests/scripts/pipeline_redirect_pipe_diff_check.sh
```

Expected: all 7 checks PASS.

Common failure modes and fixes:

- **`stdout-trunc-non-final-eof` FAIL**: Tasks 2/3 incomplete or applied incorrectly. Re-check the `let stdout_fd` blocks.
- **`final-stage-redir-no-bug` FAIL (timing)**: the `cat $T/o` after `cat | tee ...` may race with the tee write. If flaky, replace with `{ tee $T/o >/dev/null; cat $T/o; } <<< "$(printf X)"` or similar deterministic shape. If consistent in both shells but differs by timing artifact, accept the byte-difference and adjust the fragment.
- **`redir-failure-still-skips` FAIL**: the `grep -oE 'No such file or directory' | head -1` normalization may need adjustment. The point of the test is: bash AND huck BOTH print SOMETHING containing "No such file or directory" on stderr; the downstream stage produces nothing because the upstream failed before forking. If they diverge on whether stdout is empty or has the X leak, the bug isn't fixed in this path.

If any check fails because the actual bug isn't fully fixed, investigate by adding a manual reproducer to a `/tmp/repro.sh` and running it under both shells side by side.

- [ ] **Step 3: Run full suite + clippy + existing harness sweep**

```bash
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        tail -10 /tmp/h.out
    fi
done
```

Expected: green; clippy clean; ALL existing harnesses pass. Specifically watch for `pipe_compound_redirect_diff_check.sh`, `captured_pipeline_drain_diff_check.sh`, `pipefail_diff_check.sh`, `subshell_pipeline_position_diff_check.sh`, `sigpipe_diff_check.sh`, `builtin_pipe_flush_diff_check.sh`, `heredoc_pipeline_diff_check.sh` — the pipeline-adjacent harnesses.

If any pre-existing harness FAILS, investigate. The fix is additive (adds a pipe creation in a previously-skipped path) and shouldn't break anything; if it does, the cause is likely the file-fd close in the Err arm (don't close it if it was already moved to the child) or fd ordering.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/pipeline_redirect_pipe_diff_check.sh
git commit -m "$(cat <<'EOF'
v212 task 4: bash-diff harness for M-125 fix + adjacent guards

7 fragments: 3 cover the bug fix (stdout truncate, append, 3-stage
mid-pipeline redirect); 3 cover adjacent paths that must keep working
(stderr-only redirect, dup-to-stderr, final-stage redirect); 1 covers
the redirect-open-failure error path (downstream sees no stdin leak).
Each fragment carries its own printf X | feed so the bug surfaces:
without the fix, the downstream cat in the trunc/append/3-stage cases
prints X (parent stdin leak).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Final sweep — docs, full verify, stop before merge

**Files:**
- Modify: `docs/bash-divergences.md` — delete M-125; Tier 2 11 → 10.

- [ ] **Step 1: Delete M-125 from `bash-divergences.md`**

```bash
grep -n 'M-125' docs/bash-divergences.md
```

Delete the entire M-125 bullet item (a single multi-line paragraph). The pre-edit text begins with "- **M-125: a non-final pipeline stage with an explicit stdout redirect doesn't get an inter-stage pipe** — `[deferred]` low ..." and continues for ~5 lines until just before the next `- **` entry. Use the Edit tool to remove the entire bullet.

Per the docs/bash-divergences.md current-divergences-only policy: DELETE, do NOT flip to `[fixed v212]`.

- [ ] **Step 2: Update Tier 2 count**

```bash
grep -n 'Tier 2' docs/bash-divergences.md | head -3
```

After v211, Tier 2 = 11. After v212 deletes M-125, Tier 2 = 10. Update the count cell.

Verify by re-reading the file's summary section to confirm what the count actually was pre-delete — if it's not 11 (e.g. another iteration changed it), use the actual current count - 1.

- [ ] **Step 3: Final full-suite + harness sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet

bash tests/scripts/pipeline_redirect_pipe_diff_check.sh

# All existing harnesses:
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        tail -10 /tmp/h.out
    fi
done

# Headless CLI smoke:
./target/release/huck -c 'echo hello'
echo "exit=$?"

# The bug reproducer (must print empty, exit 0):
printf 'hello\n' | ./target/release/huck -c 'echo upstream > /tmp/m125-final | cat'
echo "exit=$?"
```

Expected: all green; release build clean; all harnesses pass; smoke prints `hello` + `exit=0`; the bug reproducer prints empty stdout + `exit=0`.

If any pre-existing harness FAILS at this point, investigate. v212 is a precise localized fix; it shouldn't break anything.

- [ ] **Step 4: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
v212 task 5: remove M-125 from bash-divergences.md

M-125 (non-final pipeline stage with explicit stdout redirect leaks
parent stdin to downstream) is fully resolved by v212's
make_orphan_pipe_for_eof_reader helper wired into both run_pipeline
and run_multi_stage. Tier 2 count: 11 → 10.

Per current-divergences-only policy, the M-125 entry is removed
entirely (history lives in git + iteration memory).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Stop — do NOT merge**

The final whole-branch code review is the controller's call after Task 5. Stop after this commit.

---

## Self-review

**Spec coverage:**
- `make_orphan_pipe_for_eof_reader` helper: Task 1.
- Unit test on helper: Task 1.
- Fix in `run_pipeline` (executor.rs:3085): Task 2.
- Fix in `run_multi_stage` (executor.rs:5800): Task 3.
- 7-fragment bash-diff harness: Task 4.
- Existing harness regression sweep: Task 4 step 3 + Task 5 step 3.
- Delete M-125 entry + Tier 2 update: Task 5.
- Final verify + smoke: Task 5.

**Placeholder scan:**
- No "TBD" / "implement later" / "fill in details" — every step has concrete code or an exact command.
- Task 3 Step 2 says `/* leave the existing Capture/Terminal else-branch UNCHANGED … */` inside the code block — that's an instruction to the implementer to NOT paste the irrelevant chunk; the actual replacement is bounded to the `if let Some(fd)` arm. Acceptable.
- Task 4 Step 2 lists "common failure modes and fixes" — that's diagnostic guidance, not a placeholder.

**Type consistency:**
- `make_orphan_pipe_for_eof_reader() -> io::Result<RawFd>` — same signature in Task 1 definition, Task 2 caller, Task 3 caller.
- Cleanup-arm shape in Task 2's run_pipeline differs from Task 3's run_multi_stage (cleanup_partial_pipeline_raw call vs not) — both explicitly documented as mirroring their respective existing make_pipe()-Err arms.
- The new `unsafe { libc::close(fd); }` (closing the open file fd on the helper-Err path) is consistent across both tasks.

**5 tasks. ~30 LOC of production logic + ~15 LOC of tests + ~50 LOC of harness.** Comparable to v209's task 1+2; smallest iteration since v204.
