# Subshell-pipeline tty deadlock (M-104) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `( cmd1 | cmd2 )` (a multi-stage pipeline inside a subshell) from deadlocking when huck has a controlling terminal.

**Architecture:** A forked subshell must not perform interactive job-control process-grouping for its inner pipeline. Add `Shell.in_subshell`, set it in the subshell child, and gate `run_multi_stage`'s job-control path on `!in_subshell` (the non-job-control path already works in script mode). No parser/AST change.

**Tech Stack:** Rust. `src/shell_state.rs`, `src/executor.rs`. Tests: `cargo test --bin huck`, `cargo test --test <name>` (incl. a pty test), `bash tests/scripts/*`.

**Spec:** `docs/superpowers/specs/2026-06-08-subshell-pipeline-jobctl-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `Shell` struct, `pub is_interactive: bool` ~`src/shell_state.rs:268`; the `Shell { … }` initializer(s) (grep `is_interactive:` to find every construction site that needs the new field — there may be a `Shell::new` and test constructors).
- `run_multi_stage` ~`src/executor.rs:2983`; `let interactive = matches!(sink, StdoutSink::Terminal);` ~`:2990`.
- `fork_and_run_in_subshell` ~`src/executor.rs:4090`; child branch `if pid == 0 {` with the `SIGTSTP/SIGTTIN/SIGTTOU → SIG_DFL` + `setpgid(0, pgid_target)` block (~`:4108`); the child dispatches into the command body (`run_command`/`execute`) further down.

---

## Task 1: `in_subshell` flag + gate the job-control grouping + tests

**Files:** `src/shell_state.rs`, `src/executor.rs`, a new pty test, a non-pty integration test.

- [ ] **Step 1: Write the failing pty regression test**

Read `tests/pty_interactive.rs` to copy its pty-spawn helper (it forks a pty, spawns `./target/debug/huck`, writes input, reads output with a timeout). Create `tests/subshell_pipeline_pty.rs` (or add to `tests/pty_interactive.rs`). The test: spawn huck under a pty, send `( echo hi | cat )\n` then `echo DONE_MARK\n` then `exit\n`, and assert both `hi` and `DONE_MARK` appear within ~5s.

If the existing harness exposes a simpler primitive, model on it. A self-contained version using `nix`/`libc` pty fork is acceptable if the repo already depends on it (check `Cargo.toml`; `tests/pty_interactive.rs` shows the available pty mechanism — reuse it). Example shape (adapt to the repo's harness):

```rust
// Spawn huck in a pty, drive it, return combined output (or detect a hang).
#[test]
fn subshell_pipeline_does_not_hang_on_tty() {
    let out = run_in_pty(
        &["( echo hi | cat )", "echo DONE_MARK", "exit"],
        std::time::Duration::from_secs(5),
    );
    assert!(out.contains("hi"), "pipeline output missing: {out}");
    assert!(out.contains("DONE_MARK"), "subshell hung (no DONE_MARK): {out}");
}

#[test]
fn subshell_multistage_pipeline_does_not_hang_on_tty() {
    let out = run_in_pty(
        &["( echo hi | head -n 1 | tail -n 1 )", "echo DONE2", "exit"],
        std::time::Duration::from_secs(5),
    );
    assert!(out.contains("DONE2"), "multistage subshell hung: {out}");
}
```
Build, run `cargo test --test subshell_pipeline_pty 2>&1 | tail` → FAIL (hangs → no DONE_MARK within the timeout; ensure the harness imposes the timeout so the test FAILS rather than hanging the suite).

NOTE: if a robust pty harness is hard to reuse, at minimum add the non-pty equivalence test (Step 5) which still guards the AST/exec path, and verify the tty fix manually with the spec's pty repro — but a pty regression test is strongly preferred since the bug is tty-only.

- [ ] **Step 2: Add `Shell.in_subshell`**

In `src/shell_state.rs`, add to the `Shell` struct (near `is_interactive`):
```rust
    /// True when this process is a forked subshell child. A subshell must NOT
    /// perform interactive job-control process-grouping for its inner pipelines
    /// (that deadlocks on a controlling terminal — M-104).
    pub in_subshell: bool,
```
Initialize it `false` at every `Shell` construction site (grep `is_interactive:` to find them: `Shell::new`/`Default`/test constructors). It is a plain bool copied by the child's forked address space — no special handling.

- [ ] **Step 3: Set it in the subshell child**

In `fork_and_run_in_subshell` (`src/executor.rs`), in the `if pid == 0 {` child branch, AFTER the `setpgid`/`dup2`/fd-close setup and BEFORE the child dispatches the command body (the `run_command`/`execute` call), set:
```rust
        shell.in_subshell = true;
```
(The child has its own forked copy of `*shell`; the parent is unaffected. Find the exact spot just before the command is run — read the function's child branch to the dispatch.)

- [ ] **Step 4: Gate the job-control grouping**

In `run_multi_stage` (`src/executor.rs:2990`), change:
```rust
    let interactive = matches!(sink, StdoutSink::Terminal);
```
to:
```rust
    // Job-control process-grouping (setpgid into a foreground-style group) is
    // only correct in the top-level shell. Inside a forked subshell it puts the
    // inner pipeline in a background group with default SIGTTOU/SIGTTIN handling,
    // deadlocking the subshell's wait on a controlling terminal (M-104). A
    // subshell's inner pipeline takes the non-job-control path (stages stay in
    // the subshell's process group), matching bash.
    let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_subshell;
```

- [ ] **Step 5: Non-pty equivalence tests**

Add to a new `tests/subshell_pipeline_integration.rs` (copy the `run` helper from `tests/set_x_integration.rs`):
```rust
#[test]
fn subshell_pipeline_output_unchanged() {
    assert_eq!(run("( echo hi | cat )\necho done\n").0, "hi\ndone\n");
}
#[test]
fn subshell_multistage_pipeline_output() {
    assert_eq!(run("( printf 'a\\nb\\nc\\n' | head -n 2 | tail -n 1 )\n").0, "b\n");
}
#[test]
fn subshell_pipeline_in_command_sub() {
    assert_eq!(run("x=$( ( echo hi | cat ) ); echo \"[$x]\"\n").0, "[hi]\n");
}
#[test]
fn pipestatus_inside_subshell() {
    // $PIPESTATUS still correct on the non-job-control path.
    assert_eq!(run("( false | true ); ( true | false ); echo done\n").2, 0);
}
```
Verify each against bash. Run `cargo test --test subshell_pipeline_integration 2>&1 | tail` → pass.

- [ ] **Step 6: Run the fix + verify the repro**

- `cargo build` then re-run `cargo test --test subshell_pipeline_pty 2>&1 | tail` → now PASS (DONE_MARK appears).
- If a residual hang remains on the pty test, apply the spec's **Section 4** fallback: in the subshell child, do NOT reset `SIGTTOU`/`SIGTTIN` to `SIG_DFL` (keep the parent's dispositions; the inner pipeline's re-forked/exec'd stages reset their own signals at exec). Make the smallest change that turns the pty test green. Re-verify the must-not-regress cases below.
- `cargo test 2>&1 | tail -20` → FULL suite green (especially `tests/pty_interactive.rs` — Ctrl-C/Ctrl-Z, job control, select).
- `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 7: Manual tty smoke (not committed)**
```bash
# bug repro must now complete:
python3 - <<'PY'
import os,pty,select,time
pid,fd=pty.fork()
if pid==0: os.execv("./target/debug/huck",["./target/debug/huck"])
os.write(fd,b"( echo hi | cat )\necho SMOKE_OK\nexit\n")
end=time.time()+6;buf=b""
while time.time()<end:
    r,_,_=select.select([fd],[],[],0.3)
    if r:
        try:d=os.read(fd,4096)
        except OSError:break
        if not d:break
        buf+=d
        if b"SMOKE_OK" in buf:break
os.kill(pid,9)
print("SMOKE:", "OK" if b"SMOKE_OK" in buf else "HANG")
PY
```
Expected `SMOKE: OK`. Also if `~/.nvm/nvm.sh` exists, a pty huck that does `. ~/.nvm/nvm.sh; nvm_resolve_alias default; echo RA_OK` should print `RA_OK` (no hang).

- [ ] **Step 8: Commit**
```bash
git add src/shell_state.rs src/executor.rs tests/subshell_pipeline_pty.rs tests/subshell_pipeline_integration.rs
git commit -m "fix: subshell-internal pipeline no longer deadlocks on a tty (M-104)

A multi-stage pipeline inside a subshell ( a | b ) deadlocked whenever a
controlling terminal was present: fork_and_run_in_subshell resets job-control
signals and run_multi_stage setpgid'd the inner stages into a background process
group (job-control path gated on sink==Terminal). New Shell.in_subshell (set in
the subshell child) gates that grouping off inside subshells, so the inner
pipeline uses the non-job-control path (stages stay in the subshell's pgrp),
matching bash. Fixes nvm_resolve_alias -> source ~/.bashrc hang.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 1 report
**DONE/BLOCKED**, commit SHA, whether the Section-4 signal fallback was needed, the pty-test + full-suite pass lines, clippy status, and the manual smoke result.

---

## Task 2: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read structure**
`grep -n '^## Change log\|Tier 1\|Last updated\|^### M-103\|2026-06-0' docs/bash-divergences.md | head` and `grep -n 'v107' README.md`. Read the M-103 entry, the v107 change-log entry + README row, and the Tier-1 summary line. Confirm next free Tier-1 number is **M-104**.

- [ ] **Step 2: Add M-104 `[fixed v108]`**
Tier-1 (Bugs), high: a multi-stage pipeline inside a subshell `( a | b )` deadlocked on a controlling terminal — `fork_and_run_in_subshell` reset SIGTTOU/TTIN to SIG_DFL and `run_multi_stage` setpgid'd the inner stages into a background process group (job-control gated on `sink==Terminal`); fix = `Shell.in_subshell` gates the grouping off inside forked subshells (non-job-control path, matches bash). Root-caused via nvm's `nvm_resolve_alias` hanging `source ~/.bashrc`. Bump the Tier-1 count.

- [ ] **Step 3: Change-log + README row**
`2026-06-08` v108 change-log entry (style of v107): the deadlock mechanism, the `in_subshell` gate, the nvm/`source ~/.bashrc` payoff, the pty regression test, test count. v108 README iteration row after v107.

- [ ] **Step 4: Verify + commit**
`grep -n 'M-104\|fixed v108\|v108' docs/bash-divergences.md README.md` → real numbers, no placeholders.
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v108 — subshell-pipeline tty deadlock (M-104)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | tail -5` (green incl. pty suite), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass).
- [ ] Manual tty smoke (Step 7) → `SMOKE: OK`.
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files.
```
