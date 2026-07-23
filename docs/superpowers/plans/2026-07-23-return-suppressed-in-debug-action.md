# v328 — RETURN suppressed during DEBUG action Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Suppress the RETURN trap while the DEBUG trap action is executing, matching bash — the second step of the `dbg-support` sub-arc (measured to shrink its diff 1171 → 635).

**Architecture:** One guard in `fire_return_trap` (`crates/huck-engine/src/traps.rs`): if `shell.firing_traps` contains `TrapSignal::Debug`, return without firing. Reuses the v323 `firing_traps` set.

**Tech Stack:** Rust; huck-engine; bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-23-return-suppressed-in-debug-action-design.md`
Issue: [#273](https://github.com/jdstanhope/huck/issues/273)

## Global Constraints

- bash 5.2.21: RETURN is suppressed while a DEBUG action runs; NOT suppressed while an ERR action runs; RETURN-during-RETURN already guarded (v323 same-signal). DEBUG/ERR are not suppressed by this change.
- Do NOT change: the v327 functrace/extdebug gate; the extdebug decision path; DEBUG/ERR firing.
- Commit trailer; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` trap integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in the commit (bare `#N`).

---

### Task 1: Suppress RETURN while a DEBUG action is firing

**Files:**
- Modify: `crates/huck-engine/src/traps.rs` (`fire_return_trap`)
- Create: `tests/scripts/return_in_trap_action_diff_check.sh`

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/return_in_trap_action_diff_check.sh` (model on `trap_zero_diff_check.sh`), `check "label" 'frag'` comparing `bash --norc --noprofile -c "$frag"` vs `"$HUCK_BIN" -c "$frag"` byte-identical incl. `EXIT:$?`. Cases:
```sh
check "return supp in debug action"      'set -T; helper(){ return 0; }; trap "helper" DEBUG; trap "echo RET" RETURN; echo cmd'
check "return supp multi-cmd debug"      'set -T; helper(){ return 0; }; trap "helper; :" DEBUG; trap "echo RET" RETURN; echo cmd'
check "return fires in err action"       'set -T; helper(){ return 0; }; trap "helper; echo inerr" ERR; trap "echo RET" RETURN; false'
check "real return still fires (-T)"      'set -T; f(){ echo body; }; trap "echo RET" RETURN; f'
check "real return still fires (extdbg)"  'shopt -s extdebug; f(){ echo body; }; trap "echo RET" RETURN; f'
check "print_debug_trap shape"            'set -T; pdt(){ echo "dbg $1"; }; prt(){ echo "ret $1"; }; trap "pdt \$LINENO" DEBUG; trap "prt \$LINENO" RETURN; g(){ echo g; }; g'
check "no functrace: no return"           'f(){ echo body; }; trap "echo RET" RETURN; f'
```
Build (`cargo build -p huck`) and run — the "return supp in debug action" / multi-cmd / print_debug_trap cases FAIL (huck fires the spurious RETURN).

- [ ] **Step 2: Implement the guard**

In `fire_return_trap`, after the existing functrace/extdebug gate and BEFORE `fire_pseudo_trap`:
```rust
// #273: RETURN does not fire for a function/source that returns while the
// DEBUG trap action is executing (bash suppresses the RETURN trap for the
// duration of the DEBUG trap). ERR does NOT suppress RETURN; the same-signal
// (RETURN-during-RETURN) case is already covered by fire_pseudo_trap's guard.
if shell.firing_traps.contains(&TrapSignal::Debug) {
    return;
}
```

- [ ] **Step 3: Confirm the harness passes** vs bash (all cases).

- [ ] **Step 4: Regression + dbg-support measurement**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1     # green (update any test asserting the old double-fire)
ulimit -v 1500000
for h in return_in_trap_action functrace debug_firing_points extdebug_skip lineno_fidelity; do
  bash tests/scripts/${h}_diff_check.sh >/dev/null 2>&1 && echo "$h OK" || echo "$h FAIL"
done
for t in trap_integration trap_pseudo_signals_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done
ulimit -v 2000000
HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=90 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 200 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "dbg-support2 \|"   # PASS
HUCK_BASH_TEST_CATEGORY=dbg-support HUCK_TEST_TIMEOUT=120 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 240 bash tests/bash-test-suite/runner.sh > /tmp/dbgs.md 2>&1
SC=$(grep -oE "/tmp/huck-bash-tests[^ ]*" /tmp/dbgs.md | head -1)
echo "dbg-support diff: $(wc -l < $SC/dbg-support.diff) lines (was ~1171; expect ~635)"
```
dbg-support2 MUST stay PASS; dbg-support diff should be ~635.

- [ ] **Step 5: Full sweep**
```bash
cargo build --release -p huck
ulimit -v 1500000; timeout 550 bash tests/scripts/run_diff_checks.sh   # green (coproc flake pre-existing)
```

- [ ] **Step 6: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/traps.rs tests/scripts/return_in_trap_action_diff_check.sh
git commit -m "$(cat <<'EOF'
v328: RETURN trap suppressed while a DEBUG action is executing (#273)

bash does not fire the RETURN trap for a function/source that returns while the
DEBUG trap action runs; huck did. Guard fire_return_trap on
firing_traps.contains(Debug). ERR does not suppress RETURN. Shrinks the
dbg-support bash-suite diff 1171 -> 635.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** the one guard + harness + dbg-support measurement + regressions are all Task 1.
- **Placeholders:** none; the guard is complete.
- **Type consistency:** `firing_traps: Vec<TrapSignal>`; `TrapSignal::Debug`; `fire_return_trap` unchanged signature.
- **Scope:** RETURN-during-DEBUG suppression only; #274/caller/`$LINENO` are later sub-arc iterations; DEBUG/ERR firing unchanged.
