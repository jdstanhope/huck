# huck v138 — SIGINT aborts the running command list Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An untrapped Ctrl-C (SIGINT) aborts the running command list / function / script like bash — interactive returns to a fresh prompt with `$?`=130 (shell does NOT exit); `-c`/script exits 130; a user `INT` trap still runs and continues, `trap '' INT` still ignores.

**Architecture:** Add a 5th `ExecOutcome` variant, `Interrupted`, that propagates like `Exit` and is consumed at the top level. A `check_interrupt` helper consumes `sigint_flag` and yields `Interrupted` only when no `INT` trap is installed. It is raised at `run_andor_group` (after each command), in loops, and in `wait`/`read`; for the interactive job-control case, the foreground-wait sites set `sigint_flag` when a child dies from SIGINT.

**Tech Stack:** Rust, `libc` (`SIGINT`, `WIFSIGNALED`/`WTERMSIG`), the huck test binary (`env!("CARGO_BIN_EXE_huck")`), bash-diff harness shell scripts, an expectrl-style PTY test.

**Reference:** spec at `docs/superpowers/specs/2026-06-11-sigint-abort-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a detached HEAD lost commits in a prior iteration). Stay on the `v138-sigint-abort` branch. Every commit message ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Compiler-guided propagation strategy (read first):** Adding `ExecOutcome::Interrupted` makes the Rust compiler flag every EXHAUSTIVE `match` on an `ExecOutcome` that lacks an arm. For each one the compiler reports, add an arm that PROPAGATES it upward exactly like the `Exit` arm beside it (`ExecOutcome::Interrupted => return ExecOutcome::Interrupted,` — or, at a top-level consumer, the behavior specified in Task 1). Separately, the NON-exhaustive propagation guards written as `matches!(x, ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) | ExecOutcome::FunctionReturn(_))` are NOT compiler-flagged; find them with `grep -rn "FunctionReturn(_)" src/executor.rs src/shell.rs` and add `| ExecOutcome::Interrupted` to each (known sites: executor.rs ~159, ~200, ~310, ~1275, ~1289; shell.rs ~360). Build often.

**Build note:** the repo is large; `cargo build`/`cargo test` take a few minutes. Be patient.

---

### Task 1: Core — `Interrupted` variant, `check_interrupt`, sequence/function/nested abort, top-level consumption

**Files:**
- Modify: `src/builtins.rs` (the `ExecOutcome` enum at line 12; the `run_sourced_contents_in_sink` match ~5485)
- Modify: `src/executor.rs` (add `check_interrupt`; `run_andor_group` ~150-229; propagation guards; compiler-flagged matches)
- Modify: `src/shell.rs` (REPL match ~335-364; `run_program` match ~210-217; propagation guard ~360)
- Create: `tests/sigint_abort_integration.rs`

- [ ] **Step 1: Write the failing tests** — create `tests/sigint_abort_integration.rs`:

```rust
//! v138: an untrapped SIGINT aborts the running command list / function / script
//! like bash. Deterministic via `kill -INT $$` (sets huck's own sigint_flag — no
//! PTY/timing). Run the huck binary on a script file (an isolated child).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run a script through `huck -c <script>`; return (stdout, stderr, exit_code).
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

#[test]
fn sequence_aborts_on_sigint() {
    let (out, _e, code) = huck_c("echo a; kill -INT $$; echo b");
    assert_eq!(out, "a\n", "second command must not run; out={out:?}");
    assert_eq!(code, 130, "exit 130 expected");
}

#[test]
fn function_body_aborts_and_unwinds_caller() {
    let (out, _e, code) = huck_c("f(){ echo a; kill -INT $$; echo b; }; f; echo after");
    assert_eq!(out, "a\n", "abort unwinds through the function AND the caller; out={out:?}");
    assert_eq!(code, 130);
}

#[test]
fn nested_if_aborts() {
    let (out, _e, code) = huck_c("if true; then echo a; kill -INT $$; echo b; fi; echo c");
    assert_eq!(out, "a\n", "out={out:?}");
    assert_eq!(code, 130);
}

#[test]
fn trap_handler_runs_and_continues() {
    // A user INT trap must still run AND execution continues (no abort).
    let (out, _e, code) = huck_c("trap 'echo c' INT; echo a; kill -INT $$; echo b");
    assert_eq!(out, "a\nc\nb\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn trap_ignore_continues() {
    let (out, _e, code) = huck_c("trap '' INT; echo a; kill -INT $$; echo b");
    assert_eq!(out, "a\nb\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn legit_130_status_does_not_abort() {
    // A command returning 130 WITHOUT a SIGINT must NOT abort the list.
    let (out, _e, code) = huck_c("f(){ return 130; }; f; echo still-here");
    assert_eq!(out, "still-here\n", "out={out:?}");
    assert_eq!(code, 0);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --test sigint_abort_integration 2>&1 | tail -30`
Expected: `sequence_aborts_on_sigint`, `function_body_aborts_and_unwinds_caller`, `nested_if_aborts` FAIL (huck currently prints the second command and exits 0). `trap_*` and `legit_130_*` already PASS. Record what you see.

- [ ] **Step 3: Add the `Interrupted` variant** — `src/builtins.rs` enum (line 12):

```rust
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak(u32, i32),
    LoopContinue(u32),
    FunctionReturn(i32),
    /// v138: an untrapped SIGINT was observed — abort the running command list.
    /// Propagates like `Exit` until a top-level consumer (REPL reprompts with
    /// `$?`=130 and does NOT exit; `-c`/script exits 130).
    Interrupted,
}
```

- [ ] **Step 4: Add the `check_interrupt` helper** — in `src/executor.rs` (near the top-level helpers; make it `pub(crate)` so `builtins.rs` can call it in a later task):

```rust
/// Consumes a pending SIGINT and decides whether to abort. Returns
/// `Some(ExecOutcome::Interrupted)` when an untrapped SIGINT is pending (abort the
/// running list); `None` when no SIGINT is pending OR when a user `INT` trap
/// (handler `trap 'cmd' INT` or ignore `trap '' INT`) is installed — in which case
/// the existing trap dispatch runs the handler / no-ops the ignore and execution
/// continues, matching bash. (v138)
pub(crate) fn check_interrupt(shell: &Shell) -> Option<ExecOutcome> {
    use std::sync::atomic::Ordering;
    if shell
        .sigint_flag
        .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        if shell.trap_sigids.contains_key(&libc::SIGINT) {
            return None;
        }
        return Some(ExecOutcome::Interrupted);
    }
    None
}
```

- [ ] **Step 5: Raise it in `run_andor_group`** — `src/executor.rs` (~150-229). After the FIRST `run_command` (currently `let mut status = run_command(first, shell, sink);` at ~156) and after EACH `rest` `run_command` (~197), add the interrupt check. Place it as the first thing after obtaining `status`, before the existing `matches!(... Exit ...)` propagation block. Concretely, after line ~156:

```rust
    let mut status = run_command(first, shell, sink);
    if let Some(o) = check_interrupt(shell) {
        return o;
    }
```

and inside the `for` loop, immediately after `status = run_command(command, shell, sink);` (~197):

```rust
            status = run_command(command, shell, sink);
            if let Some(o) = check_interrupt(shell) {
                return o;
            }
```

- [ ] **Step 6: Add `| ExecOutcome::Interrupted` to the non-exhaustive propagation guards**

Run `grep -rn "FunctionReturn(_)" src/executor.rs src/shell.rs` and to each `matches!(... | ExecOutcome::FunctionReturn(_))` propagation guard add `| ExecOutcome::Interrupted`. Known sites: executor.rs ~159-160, ~200-201, ~310, ~1275-1276, ~1289-1290; shell.rs ~359-360. (Leave the `match` arms — Step 7 — to the compiler.) Example, executor.rs ~159:

```rust
        ExecOutcome::Exit(_) | ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_)
            | ExecOutcome::FunctionReturn(_) | ExecOutcome::Interrupted
```

- [ ] **Step 7: Build; add a propagating arm to every compiler-flagged exhaustive match**

Run: `cargo build 2>&1 | tail -40`
For each `error[E0004]: non-exhaustive patterns: ... ExecOutcome::Interrupted not covered`, add an arm that propagates upward like the neighboring `Exit` arm — `ExecOutcome::Interrupted => return ExecOutcome::Interrupted,` — EXCEPT at the three top-level consumers below (handle those per Steps 8-10). Repeat `cargo build` until it compiles. (Expected exhaustive sites include the loop body matches in executor.rs ~752/764 and the call-function matches; the top-level matches in shell.rs ~210/335 and builtins.rs ~5485 are handled explicitly next.)

**CRITICAL — `call_function` must PROPAGATE `Interrupted`, not convert it.** `call_function` (executor.rs ~2806) catches `FunctionReturn` and converts it to `Continue(n)`. Where its body-outcome `match` is, add `ExecOutcome::Interrupted => return ExecOutcome::Interrupted` (treat it like the `Exit` arm — pass through UNCHANGED). Do NOT fold it into the `FunctionReturn`→`Continue` arm, or the abort would be swallowed at the function boundary and `function_body_aborts_and_unwinds_caller` would fail. Likewise the pipestatus post-match (executor.rs ~2786 `Exit(_) | FunctionReturn(_) => {}`) gets `ExecOutcome::Interrupted => {}` (no pipestatus change) and returns `outcome` unchanged.

- [ ] **Step 8: Top-level — `-c`/script engine** — `src/builtins.rs` `run_sourced_contents_in_sink` match (~5485). Add:

```rust
                        ExecOutcome::Interrupted => return ExecOutcome::Interrupted,
```
(Stop executing further units; propagate up to `run_program`.)

- [ ] **Step 9: Top-level — `run_program`** — `src/shell.rs` match (~210):

```rust
        ExecOutcome::Interrupted => 130,
```

- [ ] **Step 10: Top-level — interactive REPL** — `src/shell.rs` `match outcome` (~335). Add an arm that sets `$?`=130, prints a newline to stderr, and CONTINUES the loop (does NOT exit):

```rust
                    ExecOutcome::Interrupted => {
                        let mut shell = shell_cell.borrow_mut();
                        shell.set_last_status(130);
                        eprintln!();
                    }
```
(The surrounding `match` is inside the REPL loop, so falling through reprompts. `check_interrupt` already cleared `sigint_flag`.)

- [ ] **Step 11: Run the tests**

Run: `cargo test --test sigint_abort_integration 2>&1 | tail -30`
Expected: all six PASS (`sequence_*`, `function_body_*`, `nested_if_*` now abort with rc 130 + truncated output; `trap_*` and `legit_130_*` still pass).

- [ ] **Step 12: Build + clippy + commit**

Run: `cargo clippy --all-targets 2>&1 | tail -8` → no new warnings.
```bash
git add src/builtins.rs src/executor.rs src/shell.rs tests/sigint_abort_integration.rs
git commit -m "$(printf 'feat: SIGINT aborts the running command list (sequences, functions, top level)\n\nNew ExecOutcome::Interrupted, raised by check_interrupt (gated on no INT\ntrap), propagated like Exit, consumed at the top level: REPL reprompts\nwith $?=130 (no exit); -c/script exits 130.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Loops and `wait`/`read` raise `Interrupted`

**Files:**
- Modify: `src/executor.rs` (loop flag-checks at ~742, ~823, ~995, ~1155)
- Modify: `src/builtins.rs` (`wait`/`read` `check_sigint` returns at ~3078, ~3105, ~3119, ~3221, ~3280)
- Modify: `tests/sigint_abort_integration.rs` (add loop + while-read tests)

- [ ] **Step 1: Add the failing tests** — append to `tests/sigint_abort_integration.rs`:

```rust
#[test]
fn loop_aborts_and_unwinds_sequence() {
    // The loop returns Interrupted AND the trailing `echo after` must not run.
    let (out, _e, code) = huck_c("for i in 1 2 3; do echo $i; kill -INT $$; done; echo after");
    assert_eq!(out, "1\n", "out={out:?}");
    assert_eq!(code, 130);
}

#[test]
fn while_read_loop_aborts() {
    let (out, _e, code) =
        huck_c("seq 1 3 | while read x; do echo $x; kill -INT $$; done; echo after");
    assert_eq!(out, "1\n", "out={out:?}");
    assert_eq!(code, 130);
}
```

- [ ] **Step 2: Run — note current behavior**

Run: `cargo test --test sigint_abort_integration loop_ while_read 2>&1 | tail -20`
Expected: these may already PASS if Task 1's `run_andor_group` check inside the loop body catches the SIGINT. If they pass, this task hardens the loop's own between-iteration check (Step 3) and locks in coverage. If they FAIL (e.g. the loop clears the flag first via its own `compare_exchange`), Step 3 fixes it.

- [ ] **Step 3: Convert each loop's flag-check to `check_interrupt`** — `src/executor.rs`. Each loop currently has, at the top of its iteration:

```rust
        if shell
            .sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }
```

Replace each occurrence (at ~742 in `run_while_inner`, and the analogous checks at ~823, ~995, ~1155 covering `run_for`/C-for/`run_until` — verify by reading each) with:

```rust
        if let Some(o) = check_interrupt(shell) {
            return o;
        }
```

(This makes a SIGINT that arrives BETWEEN iterations abort and propagate as `Interrupted`, respecting an `INT` trap. The exhaustive loop-body match arms for `Interrupted` were already added in Task 1 Step 7.)

- [ ] **Step 4: Convert `wait`/`read`'s `check_sigint` returns** — `src/builtins.rs`. Each site currently reads `if check_sigint(shell) { return ExecOutcome::Continue(130); }` (~3078, 3105, 3119, 3221, 3280). Replace each with:

```rust
        if let Some(o) = crate::executor::check_interrupt(shell) {
            return o;
        }
```

Then delete the now-unused `check_sigint` helper (~3335) if nothing else references it (`grep -n check_sigint src/builtins.rs`); if other references remain, leave it.

- [ ] **Step 5: Run the tests + clippy**

Run: `cargo test --test sigint_abort_integration 2>&1 | tail -20` → all eight PASS.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs src/builtins.rs tests/sigint_abort_integration.rs
git commit -m "$(printf 'feat: loops and wait/read raise Interrupted on untrapped SIGINT\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Command-substitution abort

**Files:**
- Modify: `src/executor.rs` (`run_substitution` / `execute_capturing` propagation)
- Modify: `tests/sigint_abort_integration.rs` (add a cmd-sub test)

- [ ] **Step 1: Add the failing test** — append:

```rust
#[test]
fn command_substitution_aborts() {
    // SIGINT inside $(...) aborts; the trailing command must not run.
    let (out, _e, code) = huck_c("x=$(echo a; kill -INT $$; echo b); echo \"[$x]\"; echo after");
    assert!(!out.contains("after"), "must abort before `after`; out={out:?}");
    assert_eq!(code, 130, "out={out:?}");
}
```

- [ ] **Step 2: Run — note current behavior**

Run: `cargo test --test sigint_abort_integration command_substitution 2>&1 | tail -15`
Expected: likely already PASSES — the `$(...)` child receives the group SIGINT and dies; the parent's `sigint_flag` is set (the `kill -INT $$` targets the parent shell pid; in `$(...)` the child is a fork sharing the same `$$`? NOTE: `$$` is the PARENT shell pid even inside `$(...)`, so `kill -INT $$` signals the parent — its `sigint_flag` is set, and `run_andor_group`'s Task-1 check aborts after the assignment command). If it already passes, this task just locks in the behavior. If it FAILS (the assignment swallows the interrupt), do Step 3.

- [ ] **Step 3 (only if Step 2 failed): propagate `Interrupted` out of the assignment/expansion path**

Read how `run_single` handles a `SimpleCommand::Assign` whose RHS is a command substitution. Ensure that after the substitution, `check_interrupt(shell)` is consulted and `Interrupted` is returned from `run_single` (so `run_andor_group` propagates it). Add, in `run_single` after expansion of an assignment/command word that ran a substitution:

```rust
    if let Some(o) = check_interrupt(shell) {
        return o;
    }
```

(Place it after the expansion that may have run `$(...)`, before the command dispatches. Keep it minimal — one check covering the just-completed expansion.)

- [ ] **Step 4: Run the test + clippy**

Run: `cargo test --test sigint_abort_integration 2>&1 | tail -15` → all nine PASS.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → clean.

- [ ] **Step 5: Commit**

```bash
git add src/executor.rs tests/sigint_abort_integration.rs
git commit -m "$(printf 'feat: SIGINT inside a command substitution aborts the running list\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Interactive foreground-child trigger (WTERMSIG==SIGINT) + PTY test

**Files:**
- Modify: `src/executor.rs` (the 5 foreground-wait `WIFSIGNALED`/`WTERMSIG` sites: ~424-425, ~445-446, ~3461-3462, ~4275, ~4299-4300)
- Create: `tests/sigint_abort_pty.rs`

- [ ] **Step 1: Set `sigint_flag` when a foreground child dies from SIGINT**

At each foreground-wait site that computes `128 + libc::WTERMSIG(raw_status)` for a signaled child, immediately set the shell's interrupt flag when the terminating signal is SIGINT, so the next `check_interrupt` aborts (this is the interactive job-control case where the terminal delivered SIGINT to the child's pgroup, not to huck). Concretely, where the code does:

```rust
                        } else if libc::WIFSIGNALED(raw_status) {
                            128 + libc::WTERMSIG(raw_status)
```

change to:

```rust
                        } else if libc::WIFSIGNALED(raw_status) {
                            let sig = libc::WTERMSIG(raw_status);
                            if sig == libc::SIGINT {
                                shell.sigint_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            128 + sig
```

Apply at the single-command sites (~424, ~445, ~3461). For the pipeline-wait sites (~4275, ~4299) the function signature may not have `shell` in scope — check: if `shell` (or the `sigint_flag` Arc) is available, set it the same way; if the pipeline-wait helper does not have access to `shell`, have it RETURN whether the pipeline was SIGINT-terminated and set the flag in the caller (`run_multi_stage`/`run_pipeline`) where `shell` is in scope. Keep the status value (`128 + sig`) unchanged.

- [ ] **Step 2: Build + run the existing deterministic suite (no regression)**

Run: `cargo build 2>&1 | tail -3 && cargo test --test sigint_abort_integration 2>&1 | tail -12`
Expected: all nine still PASS (this change only adds the flag for the interactive external case; the deterministic `kill -INT $$` tests are unaffected).

- [ ] **Step 3: Write the PTY test** — create `tests/sigint_abort_pty.rs`, mirroring the structure of an existing PTY test (read one first: `ls tests/*_pty.rs` then open e.g. `tests/completion_jobcontrol_pty.rs` for the harness/skip pattern). The test must:
  - spawn huck in a PTY (interactive),
  - send a command that runs a foreground external that blocks: `sleep 5\n`,
  - send `\x03` (Ctrl-C),
  - assert the shell returns to a prompt promptly (does NOT hang, does NOT exit),
  - send `echo done $?\n` and assert it sees `done 130`,
  - skip gracefully (return early) if a PTY cannot be allocated, exactly like the sibling `*_pty.rs` tests.

Use the same PTY crate/helpers the sibling tests use (do not introduce a new dependency). Add a second case: a shell-function `while` loop (`f(){ while true; do :; done; }; f`), Ctrl-C, then `echo back $?` → `back 130`, shell alive.

- [ ] **Step 4: Run the PTY test**

Run: `cargo test --test sigint_abort_pty 2>&1 | tail -20`
Expected: PASS (or graceful SKIP if no PTY in the environment — never a FAIL/hang). If it hangs, that's a real bug — investigate, do not increase timeouts to mask it.

- [ ] **Step 5: Commit**

```bash
git add src/executor.rs tests/sigint_abort_pty.rs
git commit -m "$(printf 'feat: a foreground child killed by SIGINT aborts the list (job-control case) + PTY test\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Bash-diff harness (the 58th)

**Files:**
- Create: `tests/scripts/sigint_abort_diff_check.sh`

- [ ] **Step 1: Write the harness** — create `tests/scripts/sigint_abort_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v138: an untrapped SIGINT aborts the
# running command list; a user INT trap runs and continues; `trap '' INT`
# ignores. Deterministic via `kill -INT $$` (no PTY/timing). Each fragment is run
# as a FILE-ARG (an isolated child); stdout AND exit code are compared.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
TMP=$(mktemp -d)
check() {
    local label="$1" frag="$2" f="$TMP/frag.sh"
    printf '%s\n' "$frag" >"$f"
    local bo bc ho hc
    bo=$(bash "$f" 2>/dev/null); bc=$?
    ho=$("$HUCK_BIN" "$f" 2>/dev/null); hc=$?
    if [[ "$bo" == "$ho" && "$bc" == "$hc" ]]; then
        printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s (bash rc=%s huck rc=%s)\n' "$label" "$bc" "$hc"
        diff <(printf '%s' "$bo") <(printf '%s' "$ho") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}
check "sequence abort"   'echo a; kill -INT $$; echo b'
check "loop abort"       'for i in 1 2 3; do echo $i; kill -INT $$; done; echo after'
check "function abort"   'f(){ echo a; kill -INT $$; echo b; }; f; echo after'
check "nested if abort"  'if true; then echo a; kill -INT $$; echo b; fi; echo c'
check "trap handler"     'trap "echo c" INT; echo a; kill -INT $$; echo b'
check "trap ignore"      'trap "" INT; echo a; kill -INT $$; echo b'
check "legit 130 no abort" 'f(){ return 130; }; f; echo still-here'
rm -rf "$TMP"
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: chmod + build + run**

Run: `chmod +x tests/scripts/sigint_abort_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/sigint_abort_diff_check.sh`
Expected: `Total: 7, Pass: 7, Fail: 0`. If a check FAILs, paste the diff and STOP (a real bash divergence matters) — do not weaken assertions.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/sigint_abort_diff_check.sh
git commit -m "$(printf 'test: 58th bash-diff harness for SIGINT abort\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 6: Docs

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Confirm Tier-1 stays 0**

v138 fixes a real bug in the same iteration, so there is no STANDING Tier-1 entry. Read the Summary table; confirm "Bugs (Tier 1)" is `0` and leave it. If a transient note about SIGINT-not-aborting existed, ensure it is removed (it does not — the bug was found this session and not catalogued).

- [ ] **Step 2: No new deferred entry expected**

The behavior now matches bash for the in-scope cases. Do NOT invent a divergence entry. If during implementation a genuine residual surfaced (e.g. a specific nested construct that still doesn't abort), add a brief `[deferred]`/`[low]` `L-33` entry describing it and bump the Tier-4 count by 1; otherwise make no change and this task is a no-op confirmation.

- [ ] **Step 3: Commit (only if a residual entry was added)**

```bash
git add docs/bash-divergences.md
git commit -m "$(printf 'docs: note v138 SIGINT-abort residual (Lnn)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```
(If Steps 1-2 made no change, skip the commit and report this task as a no-op.)

---

### Task 7: Full regression

**Files:** none (verification only)

- [ ] **Step 1: Full suite**

Run: `cargo test 2>&1 | tail -30`
Expected: ALL pass (baseline after v137 was 3025 tests + 4 harness scripts; v138 adds the `sigint_abort_integration` tests + the PTY test). Zero failures. Paste any failure.

- [ ] **Step 2: Job-control / trap / PTY suites explicitly (the paths v138 touches)**

Run: `cargo test --test pty_interactive --test subshell_pipeline_pty --test completion_jobcontrol_pty --test subshell_tty_pty --test sigint_abort_pty 2>&1 | tail -25`
Run: `cargo test trap 2>&1 | tail -15`
Expected: pass (or graceful PTY skip). The existing `trap` tests MUST stay green (the `INT`-trap gating).

- [ ] **Step 3: ALL bash-diff harnesses**

Run: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" | tail -1; done`
Expected: every harness ends with `Fail: 0` (including the new `sigint_abort_diff_check.sh` → `Pass: 7, Fail: 0`).

- [ ] **Step 4: Clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -8`
Expected: clean.

- [ ] **Step 5: Payoff check (the original report)**

Build release: `cargo build --release 2>&1 | tail -2`. Then verify interactively (or describe to the controller for a manual check): running `nvm ls` via `~/.nvm/nvm.sh` and pressing Ctrl-C now returns to the prompt promptly instead of running to completion. (Do NOT source `~/.bashrc` — it holds credentials.) A non-interactive proxy: `target/release/huck -c 'export NVM_DIR="$HOME/.nvm"; . "$NVM_DIR/nvm.sh"; for i in $(seq 1 5); do echo $i; kill -INT $$; done'` aborts at `1` with rc 130.

- [ ] **Step 6: Commit (only if a verification-driven fix was needed)**

If Steps 1-4 surfaced a real issue, make the SMALLEST fix, re-run, and commit with the standard trailer. Otherwise no commit — verification only.

---

## Notes for the implementer
- **The abort is driven by the SIGINT flag, never by the 130 status value** — a command that legitimately returns 130 must not abort the list (`legit_130_status_does_not_abort` guards this).
- **`Interrupted` propagates like `Exit`** at every intermediate level; only the three top-level consumers (REPL, `run_program`, `run_sourced_contents_in_sink`) handle it specially.
- **Trap gating is mandatory** — `check_interrupt` returns `None` when an `INT` trap is installed, so `trap 'cmd' INT` and `trap '' INT` keep working (regression-tested).
- **Do not introduce a new PTY dependency** — reuse whatever the existing `tests/*_pty.rs` use; skip gracefully without a PTY.
- **If a PTY suite cannot run**, confirm it SKIPS rather than fails; never weaken an assertion to force a pass.
