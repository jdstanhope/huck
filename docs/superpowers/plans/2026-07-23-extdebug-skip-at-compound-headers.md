# v326 — extdebug skip/return at compound headers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the v324 compound-header DEBUG fires honor the `DebugDecision` under `shopt -s extdebug` — a non-zero DEBUG status skips the pending unit, status 2 in a subroutine simulates `return` — matching bash across for/select/case headers and arith-for init/cond/step.

**Architecture:** Replace `let _ = crate::traps::fire_debug_trap(shell);` at each compound-header fire in `executor.rs` with a `match` on the decision. `ReturnFromSub(n)` → `return ExecOutcome::FunctionReturn(n)` everywhere; `SkipCommand` is per-site (break the loop / skip the case / skip an arith expression / exit the loop).

**Tech Stack:** Rust; huck-engine executor; bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-23-extdebug-skip-at-compound-headers-design.md`
Issue: [#262](https://github.com/jdstanhope/huck/issues/262)

## Global Constraints

- bash 5.2.21 fidelity. `SkipCommand` per site: for/select header → `break` the iteration loop (skip this iteration + the rest); case header → `return Continue(shell.last_status())`; arith-for init → skip the init eval, keep looping; arith-for cond → `break` (exit loop); arith-for step → skip the step eval, keep looping. `ReturnFromSub(n)` → `return ExecOutcome::FunctionReturn(n)` at every site.
- Do NOT change: the fires' COUNTS or `$LINENO` (v324/v325 — the v325 `current_lineno` header-line stamp stays immediately before each fire); the body-command decision path (v322); pipeline-stage fires (`let _ = …` unchanged — #268).
- Without extdebug or with a zero DEBUG status, every site gets `Proceed` and behaves exactly as today.
- Commit trailer; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` trap integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in the commit (bare `#N`).

---

### Task 1: Honor the decision at the five compound-header fire sites

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — `run_for_inner`, `run_select_inner`, `run_case_inner`, `run_arith_for_inner` (the fire sites added in v324, line-stamped in v325).
- Create: `tests/scripts/extdebug_skip_diff_check.sh`

Import note: `crate::traps::DebugDecision` variants — reference as
`crate::traps::DebugDecision::{Proceed, SkipCommand, ReturnFromSub}` (or `use`
it locally).

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/extdebug_skip_diff_check.sh` (model on `trap_zero_diff_check.sh`), `check "label" 'frag'` comparing `bash --norc --noprofile -c "$frag"` vs `"$HUCK_BIN" -c "$frag"` byte-identical incl. `EXIT:$?`. The DEBUG action is a helper FUNCTION returning a controlled status (a bare `return` in a top-level action errors). Cases:
```sh
check "for-header skip iter1" 'shopt -s extdebug; tr(){ [[ $BASH_COMMAND == for\ x* ]] && return 1; return 0; }; trap tr DEBUG; for x in 1 2 3; do echo b$x; done; echo after'
check "for-header skip iter2" 'shopt -s extdebug; tr(){ [[ $BASH_COMMAND == for\ x* && $x == 1 ]] && return 1; return 0; }; trap tr DEBUG; for x in 1 2 3; do echo b$x; done; echo after'
check "for-body skip (v322)"  'shopt -s extdebug; tr(){ [[ $BASH_COMMAND == echo\ b2 ]] && return 1; return 0; }; trap tr DEBUG; for x in 1 2 3; do echo b$x; done; echo after'
check "select-header skip"    'shopt -s extdebug; tr(){ [[ $BASH_COMMAND == select* ]] && return 1; return 0; }; trap tr DEBUG; select x in a b; do echo $x; break; done <<< 1; echo after'
check "case-header skip"      'shopt -s extdebug; tr(){ [[ $BASH_COMMAND == case* ]] && return 1; return 0; }; trap tr DEBUG; case a in a) echo m;; esac; echo after'
check "arith init skip"       'shopt -s extdebug; tr(){ [[ $BASH_COMMAND == *"i=0"* ]] && return 1; return 0; }; trap tr DEBUG; for ((i=0;i<3;i++)); do echo b$i; done; echo after'
check "arith cond skip"       'shopt -s extdebug; tr(){ [[ $BASH_COMMAND == *"i<3"* ]] && return 1; return 0; }; trap tr DEBUG; for ((i=0;i<3;i++)); do echo b$i; done; echo after'
check "arith step skip"       'shopt -s extdebug; c=0; tr(){ [[ $BASH_COMMAND == *"i++"* ]] && return 1; return 0; }; trap tr DEBUG; for ((i=0;i<3;i++)); do echo b$i; c=$((c+1)); [[ $c -gt 4 ]] && break; done; echo after'
check "return2 in fn"         'shopt -s extdebug; tr(){ [[ $BASH_COMMAND == for\ x* ]] && return 2; return 0; }; trap tr DEBUG; f(){ echo pre; for x in 1 2; do echo b$x; done; echo post; }; f; echo rc=$?'
check "no-extdebug no skip"   'tr(){ [[ $BASH_COMMAND == for\ x* ]] && return 1; return 0; }; trap tr DEBUG; for x in 1 2; do echo b$x; done; echo after'
```
> NOTE: these fragments use `$BASH_COMMAND`, which huck does NOT implement. That means huck's `tr` condition never matches → the DEBUG action always returns 0 → no skip. So the harness as written would show huck NOT skipping and bash skipping. To gate on the DECISION (status) rather than `$BASH_COMMAND`, drive the skip with a **counter** in the action instead (e.g. `n=$((n+1)); [[ $n == 2 ]] && return 1`), which both shells support, and pick the counter value that lands on the intended fire. Verify each fragment triggers the intended site by first confirming bash's output, then make huck match. Author the counter-based fragments so bash and huck agree post-fix; every check must PASS. (Do NOT rely on `$BASH_COMMAND`.)

Build (`cargo build -p huck`) and run — the compound-header cases FAIL (huck ignores the decision → runs the construct anyway).

- [ ] **Step 2: Implement the five sites**

In `run_for_inner` and `run_select_inner`, at the per-iteration fire (after the v325 `current_lineno` stamp):
```rust
match crate::traps::fire_debug_trap(shell) {
    crate::traps::DebugDecision::Proceed => {}
    crate::traps::DebugDecision::SkipCommand => break,
    crate::traps::DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
}
```
(`break` exits the iteration loop; the function returns its accumulated `last`.)

In `run_case_inner`, at the entry fire:
```rust
match crate::traps::fire_debug_trap(shell) {
    crate::traps::DebugDecision::Proceed => {}
    crate::traps::DebugDecision::SkipCommand => return ExecOutcome::Continue(shell.last_status()),
    crate::traps::DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
}
```

In `run_arith_for_inner`:
- **init fire**: introduce `let mut run_init = true;`, and
  ```rust
  match crate::traps::fire_debug_trap(shell) {
      crate::traps::DebugDecision::Proceed => {}
      crate::traps::DebugDecision::SkipCommand => run_init = false,
      crate::traps::DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
  }
  ```
  then guard the existing init eval: `if run_init && let Some(init) = &clause.init { … }`.
- **cond fire**:
  ```rust
  match crate::traps::fire_debug_trap(shell) {
      crate::traps::DebugDecision::Proceed => {}
      crate::traps::DebugDecision::SkipCommand => break,
      crate::traps::DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
  }
  ```
- **step fire**: `let mut run_step = true;` (per iteration), set `run_step = false` on `SkipCommand`, and guard the existing step eval `if run_step && let Some(step) = &clause.step { … }`.

Keep the v325 `if clause.line != 0 { shell.current_lineno = shell.line_base() + clause.line; }` immediately before each fire.

- [ ] **Step 3: Confirm the harness passes** vs bash (all skip/return/no-extdebug cases). Iterate the counter-based fragments until bash == huck.

- [ ] **Step 4: Regression**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1     # green
cargo build -p huck && cargo build --release -p huck
ulimit -v 1500000; bash tests/scripts/debug_firing_points_diff_check.sh   # counts unchanged
ulimit -v 1500000; bash tests/scripts/lineno_fidelity_diff_check.sh       # $LINENO unchanged
ulimit -v 2000000; HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=60 \
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 timeout 150 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "dbg-support2 \|"   # PASS
for t in trap_integration trap_pseudo_signals_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done
ulimit -v 1500000; timeout 500 bash tests/scripts/run_diff_checks.sh      # green (coproc flake pre-existing)
```

- [ ] **Step 5: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/extdebug_skip_diff_check.sh
git commit -m "$(cat <<'EOF'
v326: honor extdebug DEBUG skip/return at compound headers (#262)

The v324 compound-header DEBUG fires ignored the DebugDecision. Honor it:
for/select header -> break the loop; case header -> skip the case; arith-for
init/step -> skip that expression; arith-for cond -> exit the loop; and
ReturnFromSub -> FunctionReturn at every site. Body commands (v322) and
pipeline stages (#268) are unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** the five sites + the harness + regressions are all Task 1 (spec §Design, §Testing).
- **Placeholders:** none; each site's code is concrete. The harness NOTE flags the `$BASH_COMMAND` pitfall and mandates counter-based fragments (huck lacks `$BASH_COMMAND`), with bash as the oracle.
- **Type consistency:** `fire_debug_trap -> DebugDecision`; `ExecOutcome::{Continue, FunctionReturn}`; the arith-for `run_init`/`run_step` flags gate the existing `Option`-guarded evals.
- **Scope:** compound headers only; pipeline (#268), counts/`$LINENO`, and the body path unchanged.
