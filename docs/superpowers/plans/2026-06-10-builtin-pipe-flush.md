# v129 — flush stdout before fork/spawn handoff (M-118) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop a forked builtin stage from silently dropping its trailing
unterminated output, and fix the related external-stage output-ordering bug, by
flushing Rust's `io::stdout()` `LineWriter` at every point where huck hands fd 1
to another process.

**Architecture:** Add one module-private helper `flush_stdout()` in
`src/executor.rs` and call it at four handoff sites: (1) the parent side just
before `libc::fork()` in `fork_and_run_in_subshell`, (2) the child side just
before `libc::_exit()` there, (3) the top of `spawn_external_with_fds`, and (4)
before `process.spawn()` in `run_subprocess`. No buffering strategy changes.

**Tech Stack:** Rust, libc, std::process. Tests: cargo integration tests
(`CARGO_BIN_EXE_huck`, piped-stdin `run()` helper) + a bash-diff harness.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a prior iteration lost commits to a
detached HEAD). Stay on the `v129-builtin-pipe-flush` branch; edit, build, commit
in place. Commit trailer on every commit:
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.

**Reference:** spec at `docs/superpowers/specs/2026-06-10-builtin-pipe-flush-design.md`.

---

### Task 1: `flush_stdout()` helper + in-process fork flushes (the M-118 core)

This task fixes the builtin-truncation bug for forked in-process stages (pipeline
stages and subshell bodies) AND the inherited-buffer duplication hazard. Both the
child-side flush (the fix) and the parent-side flush (prevents duplication) must
land together.

**Files:**
- Create: `tests/builtin_pipe_flush_integration.rs`
- Modify: `src/executor.rs` (add `flush_stdout()` helper; call at the `libc::fork()` site ~line 4513 and the `libc::_exit()` site, line 4595)

- [ ] **Step 1: Write the failing integration tests**

Create `tests/builtin_pipe_flush_integration.rs`:

```rust
//! v129: a forked builtin stage must flush its trailing partial line; the parent
//! must flush before forking so nothing is duplicated (M-118).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn builtin_unterminated_piped_not_truncated() {
    let (out, _e, _c) = run("printf \"%s\" abc | cat\n");
    assert_eq!(out, "abc", "out: {out:?}");
}

#[test]
fn builtin_only_last_line_unterminated_piped() {
    let (out, _e, _c) = run("printf \"x\\ny\\nz\" | cat\n");
    assert_eq!(out, "x\ny\nz", "out: {out:?}");
}

#[test]
fn builtin_unterminated_in_subshell() {
    let (out, _e, _c) = run("( printf x )\n");
    assert_eq!(out, "x", "out: {out:?}");
}

#[test]
fn no_duplication_parent_partial_then_piped_builtin() {
    // Parent's buffered "x" must flush BEFORE the fork, not be inherited+duped.
    let (out, _e, _c) = run("printf x; printf y | cat\n");
    assert_eq!(out, "xy", "out: {out:?}");
}

#[test]
fn terminated_builtin_unchanged() {
    let (out, _e, _c) = run("echo hello | cat\n");
    assert_eq!(out, "hello\n", "out: {out:?}");
}

#[test]
fn capture_subst_unaffected() {
    let (out, _e, _c) = run("v=$(printf \"%s\" abc); echo \"[$v]\"\n");
    assert_eq!(out, "[abc]\n", "out: {out:?}");
}

#[test]
fn loop_of_builtins_unterminated_piped() {
    let (out, _e, _c) = run("for i in 1 2 3; do printf \"$i\"; done | cat\n");
    assert_eq!(out, "123", "out: {out:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test builtin_pipe_flush_integration 2>&1 | tail -20`
Expected: `builtin_unterminated_piped_not_truncated`,
`builtin_only_last_line_unterminated_piped`, `builtin_unterminated_in_subshell`,
`no_duplication_parent_partial_then_piped_builtin`, and
`loop_of_builtins_unterminated_piped` FAIL (huck currently drops the trailing
partial line / mis-orders); `terminated_builtin_unchanged` and
`capture_subst_unaffected` already PASS.

- [ ] **Step 3: Add the `flush_stdout()` helper**

In `src/executor.rs`, near the top-level helpers (e.g. just above
`fn run_command`, after the `StdoutSink` definition), add:

```rust
/// Flush huck's buffered stdout (Rust wraps fd 1 in a `LineWriter`, so a trailing
/// partial line is held back) before handing fd 1 to another process. A fork
/// child would otherwise inherit — and possibly duplicate — the pending bytes,
/// and a spawned peer would otherwise race ahead of them. Call at every fork/spawn
/// handoff. `io::stderr()` is unbuffered, so it needs no equivalent.
fn flush_stdout() {
    use std::io::Write;
    let _ = io::stdout().flush();
}
```

(If `io::Write` is already imported at module scope, the inner `use` is harmless;
keep it for locality.)

- [ ] **Step 4: Add the parent-side flush before `libc::fork()`**

In `fork_and_run_in_subshell` (`src/executor.rs:4502`), insert the flush
immediately before the fork (currently `src/executor.rs:4513`):

```rust
) -> Result<i32, io::Error> {
    // Flush buffered parent stdout BEFORE forking so the child does not inherit
    // (and then re-flush, duplicating) any pending partial line, and so pending
    // parent bytes are ordered ahead of the child's output.
    flush_stdout();
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
```

- [ ] **Step 5: Add the child-side flush before `libc::_exit()`**

In the child branch of `fork_and_run_in_subshell`, immediately before the
`libc::_exit(status)` at `src/executor.rs:4595`:

```rust
        let status = status.rem_euclid(256);
        // Flush the builtin's buffered stdout to the dup2'd fd 1 (pipe or
        // terminal) before _exit — _exit bypasses Rust's flush machinery, which
        // is desired for parent-state side effects (history.save()) but would
        // otherwise drop a builtin's trailing unterminated line (M-118).
        flush_stdout();
        // _exit bypasses Drop and Rust's atexit/flush machinery, which is
        // exactly what we want: the parent's history.save() etc. must not run.
        unsafe { libc::_exit(status) };
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test builtin_pipe_flush_integration 2>&1 | tail -20`
Expected: all 7 tests PASS.

- [ ] **Step 7: Build + clippy**

Run: `cargo build 2>&1 | tail -3` (success) and
`cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 8: Commit**

```bash
git add src/executor.rs tests/builtin_pipe_flush_integration.rs
git commit -m "$(cat <<'EOF'
fix(v129): flush stdout around in-process fork (M-118 builtin truncation)

A builtin running as a forked stage (pipeline stage or subshell body) wrote its
output to Rust's io::stdout() LineWriter, then the child _exit'd without flushing
— dropping any trailing unterminated line (printf "%s" abc | cat -> empty). Flush
in the child before _exit (the fix), and in the parent before fork() so the child
does not inherit and duplicate a pending partial line.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: External-spawn flushes (ordering bug)

Fixes the sibling bug: a buffered parent partial line printed AFTER a spawned
external stage's output (`printf x; /usr/bin/printf y` → `yx`).

**Files:**
- Modify: `tests/builtin_pipe_flush_integration.rs` (add external-ordering tests)
- Modify: `src/executor.rs` (`spawn_external_with_fds` top ~4652; `run_subprocess` before spawn ~3200)

- [ ] **Step 1: Add the failing external-ordering tests**

Append to `tests/builtin_pipe_flush_integration.rs`:

```rust
#[test]
fn external_ordering_piped() {
    let (out, _e, _c) = run("printf x; /usr/bin/printf y | cat\n");
    assert_eq!(out, "xy", "out: {out:?}");
}

#[test]
fn external_ordering_bare() {
    let (out, _e, _c) = run("printf x; /usr/bin/printf y\n");
    assert_eq!(out, "xy", "out: {out:?}");
}

#[test]
fn external_ordering_in_subshell() {
    let (out, _e, _c) = run("printf x; ( /usr/bin/printf y )\n");
    assert_eq!(out, "xy", "out: {out:?}");
}
```

(`external_ordering_in_subshell` may already pass after Task 1's parent-fork
flush, since the subshell forks via `fork_and_run_in_subshell`; the other two go
through the external-spawn paths and will still fail until this task.)

- [ ] **Step 2: Run to verify the external tests fail**

Run: `cargo test --test builtin_pipe_flush_integration 2>&1 | tail -20`
Expected: `external_ordering_piped` and `external_ordering_bare` FAIL (huck prints
`yx`); the rest PASS.

- [ ] **Step 3: Flush at the top of `spawn_external_with_fds`**

In `src/executor.rs:4652`, add the flush at the very start of the function body
(before any work), so every external pipeline/background stage flushes pending
parent stdout before spawning:

```rust
) -> Result<i32, io::Error> {
    // Flush pending parent stdout before spawning an external stage so its output
    // does not race ahead of buffered parent bytes (M-118 sibling: ordering).
    flush_stdout();
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
```

- [ ] **Step 4: Flush before `process.spawn()` in `run_subprocess`**

In `run_subprocess` (`src/executor.rs:3116`), immediately before the
`match process.spawn()` at `src/executor.rs:3200`:

```rust
    // Flush pending parent stdout before spawning so the child's output is
    // ordered after buffered parent bytes (M-118 sibling: ordering).
    flush_stdout();
    match process.spawn() {
```

- [ ] **Step 5: Run to verify all pass**

Run: `cargo test --test builtin_pipe_flush_integration 2>&1 | tail -20`
Expected: all 10 tests PASS.

- [ ] **Step 6: Build + clippy**

Run: `cargo build 2>&1 | tail -3` (success);
`cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs tests/builtin_pipe_flush_integration.rs
git commit -m "$(cat <<'EOF'
fix(v129): flush stdout before spawning external stages (ordering)

A buffered parent partial line ("printf x") flushed only at parent exit, after a
spawned external stage already wrote its output — printf x; /usr/bin/printf y
printed "yx" vs bash "xy". Flush pending parent stdout at the top of
spawn_external_with_fds and before run_subprocess's spawn.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Bash-diff harness + docs (resolve M-118)

**Files:**
- Create: `tests/scripts/builtin_pipe_flush_diff_check.sh`
- Modify: `docs/bash-divergences.md` (delete M-118, decrement Tier-1 count)

- [ ] **Step 1: Write the bash-diff harness**

Create `tests/scripts/builtin_pipe_flush_diff_check.sh` (mirror the established
piped-stdin `check()` convention; our fragments contain no `!` so history
expansion is not a concern):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v129: a forked builtin stage must flush
# its trailing unterminated line, and a buffered parent partial line must be
# ordered before a spawned/forked child's output (M-118 + the ordering sibling).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "builtin unterminated piped"   'printf "%s" abc | cat'
check "only last line unterminated"  'printf "x\ny\nz" | cat'
check "builtin piped to head"        'printf "%s" abc | head'
check "two builtins first unterm"    'printf "a\nb" | tr a-z A-Z'
check "terminated builtin unchanged" 'echo hello | cat'
check "no duplication"               'printf x; printf y | cat'
check "external ordering piped"      'printf x; /usr/bin/printf y | cat'
check "external ordering bare"       'printf x; /usr/bin/printf y'
check "builtin in subshell"          '( printf x )'
check "external in subshell"         'printf x; ( /usr/bin/printf y )'
check "loop of builtins piped"       'for i in 1 2 3; do printf "$i"; done | cat'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run the harness**

Run: `bash tests/scripts/builtin_pipe_flush_diff_check.sh`
Expected: `Total: 11, Pass: 11, Fail: 0`.

- [ ] **Step 3: Delete the M-118 entry from `docs/bash-divergences.md`**

Read `docs/bash-divergences.md`. Find the M-118 entry (Tier-1 "Bugs", high). Delete
the entire entry (the doc tracks CURRENT divergences only — resolved ones are
removed, not flipped to `[fixed]`). Then decrement the Tier-1 count wherever it
appears (the summary table / section header) from 2 to 1. Search the whole doc for
any count referencing Tier-1 and update it. Do NOT touch any other entry. (Note:
there is no residual — the ordering sibling is fixed in this same iteration, so do
NOT add a new deferred entry.)

- [ ] **Step 4: Verify the doc edit**

Run: `grep -n "M-118" docs/bash-divergences.md`
Expected: no matches (entry fully removed).

- [ ] **Step 5: Full regression + clippy**

Run: `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head`
(expect no output) and `cargo clippy --all-targets 2>&1 | tail -3` (clean). Also
re-run a couple of existing harnesses as a smoke check, e.g.
`bash tests/scripts/async_list_diff_check.sh | tail -1`.

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/builtin_pipe_flush_diff_check.sh docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
test+docs(v129): builtin_pipe_flush harness; resolve M-118

Add the bash-diff harness for the flush-before-handoff discipline and delete the
now-fixed M-118 divergence (Tier-1 2->1).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-review notes
- **Spec coverage:** all four flush sites (parent-fork, child-_exit, external
  spawn x2) are implemented across Tasks 1–2; the harness + integration tests
  cover every row of both spec tables; the docs deletion is Task 3.
- **Type consistency:** `flush_stdout()` is defined once (Task 1 Step 3) and
  called by name at all four sites; `run()` test helper is identical to the
  established integration-test pattern.
- **Ordering:** Task 1 lands the child + parent in-process flushes together (the
  parent flush is required to avoid the duplication regression the child flush
  would otherwise introduce). Task 2's external flushes are independent.
