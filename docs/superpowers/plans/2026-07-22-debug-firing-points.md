# v324 — DEBUG trap firing points Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fire the DEBUG trap at the points bash 5.2.21 fires it but huck misses — pipeline stages, `for`/`select` header per iteration, arithmetic-`for` per expression, `case` header, and (under `set -T`) inside subshells — matching bash's firing counts.

**Architecture:** Add `let _ = crate::traps::fire_debug_trap(shell);` at each construct's fire point in the executor (unconditional — NOT inside the `set -x` xtrace guard, but at the same logical point the existing `xtrace_compound` calls mark). For subshells, preserve DEBUG/RETURN traps across the fork when `functrace` is set. Firing-count fidelity only.

**Tech Stack:** Rust; huck-engine executor; bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-22-debug-firing-points-design.md`
Issue: [#257](https://github.com/jdstanhope/huck/issues/257)

## Global Constraints

- bash 5.2.21 firing-count fidelity. The DEBUG action must RUN at each new point; the returned `DebugDecision` is IGNORED (`let _ =`) at these new sites (extdebug skip/return there is #262, deferred). `$LINENO` correctness of the new fires is #261 (deferred) — do NOT add clause line fields or touch the parser.
- The fire must be UNCONDITIONAL (it fires whether or not `set -x` is on). Place it next to the existing `xtrace_compound(...)` call but OUTSIDE the `if shell.shell_options.xtrace { … }` guard.
- Do NOT change: the existing simple-command / bare-assignment fire sites; `while`/`until`/`if`/`{ }` (already correct); the `fire_debug_trap` implementation; RETURN/ERR behavior.
- `fire_debug_trap` is recursion-guarded (v323 `firing_traps` set), so the DEBUG action's own commands won't re-fire — no special handling needed.
- Commit trailer on every commit; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` trap integration binaries single-threaded before push; NO GPL bash text in the repo.

## Verifying firing counts

A subshell's DEBUG-counter increments are LOST when the subshell exits, and `$LINENO` is deferred — so count fires by having the DEBUG action print a FIXED MARKER to stdout and comparing full output to bash:
```sh
trap 'echo D' DEBUG; <fragment>
```
bash vs huck output must be byte-identical.

---

### Task 1: In-process compound fires — `for`, `select`, `case`, arithmetic-`for`

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — `run_for_inner`, `run_select_inner`, `run_case_inner`, `run_arith_for_inner`.

**Interfaces:** none new. Each edit inserts one `let _ = crate::traps::fire_debug_trap(shell);` at a fire point.

Fire points (place each fire immediately AFTER the corresponding `if shell.shell_options.xtrace { xtrace_compound(...) }` block — the fire is unconditional):

- **`run_for_inner`** (~line 1814–1828): inside the `for value in values` loop, per iteration, right after the xtrace block (~line 1827) and BEFORE `try_set(&clause.var, value)` / the body. → one fire per iteration.
- **`run_select_inner`** (~line 2172): at the per-iteration point, right after its xtrace block, before the body runs. → one fire per menu iteration.
- **`run_case_inner`** (~line 2344): once at entry, right after its xtrace block, before the pattern-match loop. → one fire.
- **`run_arith_for_inner`**: three fires —
  - after the init xtrace block (~line 2013), BEFORE `eval_arith_word(init, …)`, only when `clause.init.is_some()` (an absent section is not a command).
  - after the cond xtrace block (~line 2039), BEFORE the cond `eval_arith_word`, only when `clause.cond.is_some()`.
  - after the step xtrace block (~line 2089), BEFORE the step `eval_arith_word`, only when `clause.step.is_some()`.

- [ ] **Step 1: Read the four functions** to confirm the exact insertion point (after each xtrace block, before the eval/body). The `xtrace_compound` call sites are the fire anchors.

- [ ] **Step 2: Write failing count fragments (manual, drive the red state)**

Build and compare bash vs huck for each (they should DIFFER before the fix):
```bash
cargo build -p huck
for frag in \
  'trap "echo D" DEBUG; for x in 1 2 3; do echo $x; done' \
  'trap "echo D" DEBUG; for ((i=0;i<2;i++)); do echo $i; done' \
  'trap "echo D" DEBUG; case a in a) echo m;; esac'; do
  echo "--- $frag"
  diff <(echo "$frag" | bash --norc) <(echo "$frag" | ./target/debug/huck) && echo SAME || echo DIFF
done
```
Expected: DIFF (huck fires fewer D's).

- [ ] **Step 3: Insert the fires** (`let _ = crate::traps::fire_debug_trap(shell);`) at the six points above.

- [ ] **Step 4: Verify counts match bash** (re-run Step 2's diffs → SAME for for/arith-for/case). Also spot-check `select` via a piped choice, e.g.:
```bash
echo 'trap "echo D" DEBUG; select x in a b; do echo $x; break; done' | ... # compare bash vs huck with `<<< 1`
```

- [ ] **Step 5: Regression — while/until/if/group unchanged, no double-fire**
```bash
for frag in 'trap "echo D" DEBUG; i=0; while [ $i -lt 2 ]; do echo $i; i=$((i+1)); done' \
            'trap "echo D" DEBUG; if true; then echo y; fi' \
            'trap "echo D" DEBUG; { echo a; echo b; }' \
            'trap "echo D" DEBUG; echo solo'; do
  diff <(echo "$frag" | bash --norc) <(echo "$frag" | ./target/debug/huck) && echo SAME || echo DIFF
done
```
Expected: all SAME.

- [ ] **Step 6: Full huck-engine lib suite + fmt + commit**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v324: fire DEBUG at for/select/case/arith-for headers (#257)

Add an unconditional fire_debug_trap at each compound header fire point (for &
select per iteration, case once, arith-for per init/cond/step expression),
matching bash's firing counts. Decision ignored at these sites (extdebug
skip/return there is #262); $LINENO fidelity is #261.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Pipeline stages + functrace subshells + diff harness

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — `spawn_pipeline` (~line 6316) and the subshell `clear_for_subshell` call site (~line 8215).
- Create: `tests/scripts/debug_firing_points_diff_check.sh`

**Interfaces:** none new.

- [ ] **Step 1: Pipeline stages — fire in the parent before forking each stage**

Read `spawn_pipeline` (~line 6316) and `run_multi_stage` (~line 7215). Multi-stage pipelines fork each stage; single-stage runs in-process (unchanged). In `spawn_pipeline`, in the parent, BEFORE forking each stage (in stage order), add `let _ = crate::traps::fire_debug_trap(shell);`. The fire must happen in the PARENT (bash's DEBUG output goes to the terminal, not into the pipe), once per stage. Verify no fire happens for a single-stage pipeline through this path (that path is `run_command`, not `spawn_pipeline`).

Verify:
```bash
cargo build -p huck
diff <(echo 'trap "echo D" DEBUG; echo a | cat' | bash --norc) \
     <(echo 'trap "echo D" DEBUG; echo a | cat' | ./target/debug/huck) && echo SAME
diff <(echo 'trap "echo D" DEBUG; echo solo' | bash --norc) \
     <(echo 'trap "echo D" DEBUG; echo solo' | ./target/debug/huck) && echo SAME   # single: still 1 fire
```

- [ ] **Step 2: functrace subshells — preserve DEBUG/RETURN across the fork**

At the subshell fork's `crate::traps::clear_for_subshell(shell);` (executor.rs ~8215): when `shell.shell_options.functrace` is true, preserve the DEBUG and RETURN trap actions across the reset. Implement WITHOUT changing `clear_for_subshell`'s signature — save the two actions before, restore after:
```rust
let saved_debug = if shell.shell_options.functrace {
    shell.traps.get(&crate::traps::TrapSignal::Debug).cloned()
} else { None };
let saved_return = if shell.shell_options.functrace {
    shell.traps.get(&crate::traps::TrapSignal::Return).cloned()
} else { None };
crate::traps::clear_for_subshell(shell);
if let Some(a) = saved_debug { shell.traps.insert(crate::traps::TrapSignal::Debug, a); }
if let Some(a) = saved_return { shell.traps.insert(crate::traps::TrapSignal::Return, a); }
```
(`shell.traps` is `HashMap<TrapSignal, Option<String>>`; `.get(...).cloned()` yields `Option<Option<String>>` — the outer Option is presence, restore only when present.) The subshell's body simple commands then fire DEBUG through the normal path.

Verify:
```bash
cargo build -p huck
# WITH set -T: fires inside the subshell (D per interior command)
diff <(echo 'set -T; trap "echo D" DEBUG; ( echo a; echo b )' | bash --norc) \
     <(echo 'set -T; trap "echo D" DEBUG; ( echo a; echo b )' | ./target/debug/huck) && echo SAME
# WITHOUT set -T: trap NOT inherited (regression guard)
diff <(echo 'trap "echo D" DEBUG; ( echo a; echo b )' | bash --norc) \
     <(echo 'trap "echo D" DEBUG; ( echo a; echo b )' | ./target/debug/huck) && echo SAME
```

- [ ] **Step 3: Write the bash-diff harness**

Read `tests/scripts/trap_zero_diff_check.sh` for the pattern. Create `tests/scripts/debug_firing_points_diff_check.sh` — `check "label" 'frag'` comparing `bash --norc --noprofile -c "$frag"` vs `"$HUCK_BIN" -c "$frag"` byte-identical incl. `EXIT:$?`. Every DEBUG action prints the fixed marker `D` (NOT `$LINENO`). Cases:
```sh
check "pipeline stages"       'trap "echo D" DEBUG; echo a | cat'
check "for per-iteration"     'trap "echo D" DEBUG; for x in 1 2 3; do echo $x; done'
check "arith-for expressions" 'trap "echo D" DEBUG; for ((i=0;i<2;i++)); do echo $i; done'
check "case header"           'trap "echo D" DEBUG; case a in a) echo m;; esac'
check "select header"         'trap "echo D" DEBUG; select x in a b; do echo $x; break; done <<< 1'
check "functrace subshell"    'set -T; trap "echo D" DEBUG; ( echo a; echo b )'
check "subshell no functrace" 'trap "echo D" DEBUG; ( echo a; echo b )'
check "while unchanged"       'trap "echo D" DEBUG; i=0; while [ $i -lt 2 ]; do echo $i; i=$((i+1)); done'
check "if unchanged"          'trap "echo D" DEBUG; if true; then echo y; fi'
check "group unchanged"       'trap "echo D" DEBUG; { echo a; echo b; }'
check "single command"        'trap "echo D" DEBUG; echo solo'
check "nested pipeline+for"   'trap "echo D" DEBUG; for x in 1 2; do echo $x | cat; done'
```
Adjust any fragment whose quoting/interaction is awkward so it expresses the intended construct (the gate is bash==huck parity for what you actually run). Every check must PASS.

```bash
cargo build -p huck && cargo build --release -p huck
ulimit -v 1500000; timeout 120 bash tests/scripts/debug_firing_points_diff_check.sh
```

- [ ] **Step 4: Full regression sweep + dbg-support2 + integration bins**
```bash
ulimit -v 1500000; timeout 300 bash tests/scripts/run_diff_checks.sh          # all green (new check auto-discovered)
ulimit -v 2000000; HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=60 \
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 timeout 150 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "dbg-support2 \|"   # PASS
for t in trap_integration trap_pseudo_signals_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done   # green
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
```
(If `coproc_diff_check.sh` flakes in the sweep, that's the pre-existing v317 timing flake — re-run to confirm it's unrelated.)

- [ ] **Step 5: `cargo fmt --all` and commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/debug_firing_points_diff_check.sh
git commit -m "$(cat <<'EOF'
v324: fire DEBUG at pipeline stages + inside functrace subshells (#257)

Fire DEBUG in the parent before forking each pipeline stage, and preserve
DEBUG/RETURN traps across a subshell fork under set -T so interior commands
fire. Adds debug_firing_points_diff_check.sh gating the firing counts vs bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** Task 1 = for/select/case/arith-for header fires (spec §Design 2–5). Task 2 = pipeline stages (§1), functrace subshells (§6), and the diff harness (§Testing 1). Decision-ignored + `$LINENO`/extdebug deferrals are honored (no clause line fields, `let _ =`).
- **Placeholders:** none; the one non-trivial code block (functrace save/restore) is complete; fire insertions are `let _ = crate::traps::fire_debug_trap(shell);` at named anchors.
- **Type consistency:** `fire_debug_trap(&mut Shell) -> DebugDecision` (ignored); `shell.traps: HashMap<TrapSignal, Option<String>>`; `shell.shell_options.functrace: bool` — all verified.
- **Scope:** only fire-point additions + functrace-subshell preservation; no parser changes, no decision honoring at new sites, no `$LINENO` work.
