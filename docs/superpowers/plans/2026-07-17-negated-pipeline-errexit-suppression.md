# v311 — `!`-negated pipeline suppresses errexit/ERR for its body — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `set -e; ! eval false; echo after` prints `after` (rc 0) like bash — for every inner-execution construct under `!` — without suppressing a real `exit`.

**Architecture:** In `run_pipeline`, raise the existing `err_suppressed_depth` counter around the negated pipeline's body (the same mechanism `while`/`if` conditions use), so any inner errexit/ERR check the body runs is exempted uniformly.

**Tech Stack:** Rust, the `ExecOutcome` / `err_suppressed_depth` model in `executor.rs`, a bash-diff harness.

**Spec:** `docs/superpowers/specs/2026-07-17-negated-pipeline-errexit-suppression-design.md` — read it first (has the 12-row bash table).

**Issues:** [#1](https://github.com/jdstanhope/huck/issues/1) (this fix); [#198](https://github.com/jdstanhope/huck/issues/198) (the error-fatality funnel umbrella this is the first member of).

## Global Constraints

- **Branch:** `v311-negated-errexit`. Never commit to `main`; never merge.
- **Commit trailer**, exactly: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **`cargo fmt --all`** before committing — CI enforces `cargo fmt --all --check`.
- **⚠️ NEVER run `cargo test --workspace` or a bare `cargo test`** — 1 core / 1.9 GB box; it OOM-kills the session. Per-crate: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck`.
- **⚠️ The `grep` command in this shell is BROKEN** (prints "claude native binary not installed"). Use `/usr/bin/grep` for content searches. Does not affect cargo/bash/the build.
- **The bump wraps ONLY the negated body**, inc/dec bracketing exactly that call with **no early return between them** so the counter stays balanced.
- **A real `exit` must still propagate** — do NOT touch `run_pipeline`'s existing negation of `Continue` only; `Exit` must pass through unchanged.

---

### Task 1: bump `err_suppressed_depth` around the negated pipeline body + harness

**Files:**
- Create: `tests/scripts/negated_errexit_diff_check.sh`
- Modify: `crates/huck-engine/src/executor.rs` (`run_pipeline`, currently lines 2778-2799)

**Interfaces:**
- Consumes: `Shell::err_suppressed_depth: u32` (`shell_state.rs:811`), the existing `maybe_errexit` gate (`executor.rs:173`), the existing `run_command` / `run_multi_stage` dispatch.
- Produces: nothing new for later tasks (single-task plan).

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/negated_errexit_diff_check.sh`. It byte-diffs huck vs bash (stdout+stderr+rc) on each case. `set -e` is prefixed on every fragment.

```bash
#!/usr/bin/env bash
# v311 (#1): a `!`-negated pipeline must suppress `set -e`/ERR for its WHOLE
# body, including inner executions that are not their own boundary (`eval`,
# brace groups). huck exited where bash negates-and-continues, because the
# inner failing command returned Exit via the errexit gate and bypassed the
# outer `!`. Fixed by raising err_suppressed_depth around the negated body.
#
# INVARIANTS the fix must NOT break: a real `exit` inside the body still exits
# (`! eval 'exit 5'` -> rc 5), and errexit still fires normally WITHOUT `!`.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
check() {
  local label=$1 frag=$2
  local bo be br ho he hr
  bo=$(bash -c "$frag" 2>/tmp/v311_be); br=$?; be=$(cat /tmp/v311_be)
  ho=$("$HUCK" -c "$frag" 2>/tmp/v311_he); hr=$?; he=$(cat /tmp/v311_he)
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash: out=[$bo] err=[$be] rc=$br"; echo "  huck: out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- The fix (red -> green): inner failure under `!` must be suppressed.
check 'bang-eval-false'      'set -e; ! eval false; echo after'
check 'bang-eval-exit5-neg'  'set -e; ! eval "(exit 5)"; echo after'
check 'bang-brace-false'     'set -e; ! { false; }; echo after'
check 'bang-brace-false-true' 'set -e; ! { false; true; }; echo after'
check 'bang-brace-true-false' 'set -e; ! { true; false; }; echo after'
check 'bang-eval-false-true'  'set -e; ! eval "false; true"; echo after'

# --- Controls: already-correct, must stay green.
check 'bang-false'           'set -e; ! false; echo after'
check 'bang-subshell'        'set -e; ! ( false ); echo after'
check 'bang-builtin-false'   'set -e; ! builtin false; echo after'
check 'no-bang-eval-false'   'set -e; eval false; echo after'    # must STILL exit rc 1

# --- Invariant guards.
check 'bang-eval-real-exit'  'set -e; ! eval "exit 5"; echo after'         # real exit -> rc 5, NOT suppressed
check 'err-trap-bang'        'set -e; trap "echo ERR" ERR; ! eval false; echo after'   # ERR suppressed under !
check 'err-trap-no-bang'     'set -e; trap "echo ERR" ERR; eval false; echo after'     # ERR fires without !

rm -f /tmp/v311_be /tmp/v311_he
if [ $FAIL -ne 0 ]; then echo "negated_errexit_diff_check FAILED" >&2; exit 1; fi
echo "negated_errexit_diff_check OK"
```

- [ ] **Step 2: Run the harness — verify the 6 fix cases FAIL, the rest PASS**

Run: `cargo build -q -p huck && bash tests/scripts/negated_errexit_diff_check.sh`
Expected: `FAIL [bang-eval-false]`, `FAIL [bang-eval-exit5-neg]`, `FAIL [bang-brace-false]`, `FAIL [bang-brace-false-true]`, `FAIL [bang-brace-true-false]`, `FAIL [bang-eval-false-true]`; all controls and invariant guards PASS; overall `FAILED`. This is the RED gate. If a CONTROL or an INVARIANT guard fails at RED, stop and report — the harness is wrong, or a control case is not actually already-correct.

- [ ] **Step 3: Implement — raise `err_suppressed_depth` around the negated body**

In `crates/huck-engine/src/executor.rs`, the current `run_pipeline` is:

```rust
fn run_pipeline(
    pipeline: &Pipeline,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    let outcome = if pipeline.commands.len() == 1 {
        // Single-stage pipeline: run directly in the parent shell (no fork needed).
        // This covers both Simple commands and compound commands as single stages.
        run_command(&pipeline.commands[0], shell, sink, err_sink)
    } else {
        run_multi_stage(&pipeline.commands, shell, sink, err_sink)
    };
    if pipeline.negate {
        // Negate the exit status only; $PIPESTATUS (set by the stage(s) above)
        // stays raw, and control-flow outcomes propagate unchanged.
        if let ExecOutcome::Continue(s) = outcome {
            return ExecOutcome::Continue(if s == 0 { 1 } else { 0 });
        }
    }
    outcome
}
```

Replace the body-running `let outcome = …;` with a version that raises the suppression counter for the negated case. Keep the negation logic below it unchanged:

```rust
fn run_pipeline(
    pipeline: &Pipeline,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> ExecOutcome {
    // #1: a `!`-negated pipeline's failure is EXPECTED (it is being tested), so
    // `set -e`/ERR must not fire for anything the body runs — including inner
    // executions like `eval` and brace groups that are NOT their own boundary.
    // Raise the shared errexit-suppression counter (the same mechanism a
    // while/if condition uses) around the body only; the outer and-or gate
    // already exempts the negated pipeline itself. A real `exit` still
    // propagates: it returns `ExecOutcome::Exit` directly, never through the
    // errexit gate the counter controls, and the negation below only rewrites
    // `Continue`.
    if pipeline.negate {
        shell.err_suppressed_depth += 1;
    }
    let outcome = if pipeline.commands.len() == 1 {
        // Single-stage pipeline: run directly in the parent shell (no fork needed).
        // This covers both Simple commands and compound commands as single stages.
        run_command(&pipeline.commands[0], shell, sink, err_sink)
    } else {
        run_multi_stage(&pipeline.commands, shell, sink, err_sink)
    };
    if pipeline.negate {
        shell.err_suppressed_depth -= 1;
        // Negate the exit status only; $PIPESTATUS (set by the stage(s) above)
        // stays raw, and control-flow outcomes propagate unchanged.
        if let ExecOutcome::Continue(s) = outcome {
            return ExecOutcome::Continue(if s == 0 { 1 } else { 0 });
        }
    }
    outcome
}
```

(The two `if pipeline.negate` blocks bracket the body with a balanced inc/dec and no early return between them. Do not restructure further.)

- [ ] **Step 4: Run the harness — verify GREEN**

Run: `cargo build -q -p huck && bash tests/scripts/negated_errexit_diff_check.sh`
Expected: all `PASS`, `negated_errexit_diff_check OK`. The 6 fix cases now match bash; controls and invariant guards still pass (especially `bang-eval-real-exit` → rc 5 and `no-bang-eval-false` → rc 1).

- [ ] **Step 5: No regression on the existing negation / set-e harnesses + lib tests**

Run:
```bash
bash tests/scripts/bang_negation_diff_check.sh
bash tests/scripts/bracket_negation_diff_check.sh
bash tests/scripts/set_e_andor_diff_check.sh
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```
Expected: each harness prints its `… OK`; all lib tests pass. (These cover the pre-existing `!`-negation and `set -e`/and-or behavior; the counter bump must not disturb them.)

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/negated_errexit_diff_check.sh
git commit -m "$(cat <<'EOF'
fix: a !-negated pipeline suppresses errexit/ERR for its whole body (#1)

run_pipeline only negated a Continue outcome; when a `!`-negated command ran an
inner execution (eval, brace group) the inner failure returned Exit via the
errexit gate and bypassed the `!`, exiting the shell where bash negates and
continues. (A subshell escaped this — it is an execution boundary — so the
issue's "subshell buggy" framing was stale; the real cases are `! eval` and
`! { }`.)

Fix: raise the existing err_suppressed_depth counter around the negated pipeline
body — the same mechanism while/if conditions use — so any inner errexit/ERR
check is suppressed uniformly. A real `exit` still propagates (it returns Exit
directly, not via the errexit gate), and errexit still fires normally without
`!`. First member of the #198 error-fatality funnel, fixed through its intended
vehicle rather than a new ad-hoc check.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Verification (controller, before the PR)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo build -p huck --locked` and `cargo build --release -p huck --locked`.
- [ ] `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — pass.
- [ ] The `-p huck` integration binaries touching control-flow / traps, each single-threaded with a `ulimit -v` guard: `trap_integration`, `exit_inherits_integration`, `subshell_integration`.
- [ ] `tests/scripts/run_diff_checks.sh` on both binaries — green (the new `negated_errexit_diff_check.sh` is picked up automatically; the lone known flake is `pipeline_stage_redirect_fail_diff_check.sh` case `amb-stdin-mid`, [#180](https://github.com/jdstanhope/huck/issues/180)).
- [ ] PR with `Closes #1`; **the user merges, not you.** Wait for CI to finish and pass before saying it is ready.
