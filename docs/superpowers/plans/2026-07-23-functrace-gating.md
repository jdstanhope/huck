# v327 — functrace gating Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the DEBUG and RETURN traps fire inside functions and sourced files only under `set -T` (functrace), matching bash — the dominant fix for the `dbg-support` bash-suite category.

**Architecture:** Two gates in `crates/huck-engine/src/traps.rs` — one in `fire_debug_trap` (centralizing the gate for every DEBUG fire site) and one in `fire_return_trap`. The predicate is the v322 `in_subroutine` (a `FrameKind::Function` or `FrameKind::Source` on the call stack) plus `shell_options.functrace`.

**Tech Stack:** Rust; huck-engine; bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-23-functrace-gating-design.md`
Issue: [#272](https://github.com/jdstanhope/huck/issues/272)

## Global Constraints

- bash 5.2.21 fidelity: DEBUG/RETURN fire inside a function or sourced script ONLY under `set -T`; at the top level (no Function/Source frame) they fire regardless. A gated DEBUG fire returns `DebugDecision::Proceed` (no action runs).
- Do NOT change: the extdebug decision path (v322/v326), the fires' `$LINENO` (v325) or COUNTS at the top level (v324), the subshell functrace handling (v324).
- Commit trailer; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` trap integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in the commit (bare `#N`).

---

### Task 1: Gate DEBUG/RETURN on functrace inside subroutines

**Files:**
- Modify: `crates/huck-engine/src/traps.rs` (`fire_debug_trap`, `fire_return_trap`; update any unit tests that assumed always-fire-in-function)
- Create: `tests/scripts/functrace_diff_check.sh`

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/functrace_diff_check.sh` (model on `trap_zero_diff_check.sh`), `check "label" 'frag'` comparing `bash --norc --noprofile -c "$frag"` vs `"$HUCK_BIN" -c "$frag"` byte-identical incl. `EXIT:$?`. Cases:
```sh
check "return no -T"        'f(){ echo body; }; trap "echo RET" RETURN; f'
check "return -T"           'f(){ echo body; }; trap "echo RET" RETURN; set -T; f'
check "debug in fn no -T"   'g(){ echo g; }; f(){ g; echo f; }; trap "echo D" DEBUG; f'
check "debug in fn -T"      'g(){ echo g; }; f(){ g; echo f; }; trap "echo D" DEBUG; set -T; f'
check "toggle T between"    'f(){ echo b; }; trap "echo D" DEBUG; trap "echo R" RETURN; set -T; f; set +T; f; set -T; f'
check "top-level unaffected" 'trap "echo D" DEBUG; echo one; echo two'
check "return -T then off"  'f(){ echo b; }; trap "echo R" RETURN; set -T; f; set +T; f'
```
(A sourced-file case needs a temp file — add one that creates a small sourced script and checks DEBUG fires inside it only under `-T`, and RETURN on source return under `-T`. If awkward to express via `-c`, cover source in a `check`-with-tempfile helper as in `lineno_fidelity_diff_check.sh`.)

Build (`cargo build -p huck`) and run — the in-function DEBUG/RETURN cases FAIL (huck over-fires).

- [ ] **Step 2: Implement the DEBUG gate**

In `fire_debug_trap` (traps.rs), after the `firing_traps.contains` recursion guard and BEFORE the action lookup, compute `in_subroutine` and gate:
```rust
let in_subroutine = shell.call_stack.iter().any(|f| {
    matches!(
        f.kind,
        crate::shell_state::FrameKind::Function | crate::shell_state::FrameKind::Source
    )
});
// functrace (`set -T`): DEBUG is inherited into a function or sourced script
// only under functrace; at the top level it always fires. (bash)
if in_subroutine && !shell.shell_options.functrace {
    return DebugDecision::Proceed;
}
```
Then DELETE the later duplicate `in_subroutine` computation (the one feeding `debug_decision`) and reuse this one — keep the `debug_decision(shell.extdebug(), shell.last_status(), in_subroutine)` call unchanged.

- [ ] **Step 3: Implement the RETURN gate**

In `fire_return_trap`:
```rust
pub fn fire_return_trap(shell: &mut Shell) {
    // RETURN is inherited into a function/sourced script only under functrace
    // (`set -T`); without it the trap does not fire on return. (bash)
    if !shell.shell_options.functrace {
        return;
    }
    fire_pseudo_trap(shell, TrapSignal::Return);
}
```

- [ ] **Step 4: Confirm the harness passes** vs bash (all functrace cases).

- [ ] **Step 5: Update unit tests + regression**

Some `traps.rs` unit tests may set a Function frame and assert DEBUG/RETURN fires without functrace — those encoded the OLD (wrong) behavior; update them to set `functrace = true` where they intend the trap to fire in a subroutine, or move to top-level. Then:
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1     # green
ulimit -v 1500000; bash tests/scripts/functrace_diff_check.sh    # PASS
# Proceed-path / top-level regressions (their fires are top-level):
bash tests/scripts/debug_firing_points_diff_check.sh
bash tests/scripts/extdebug_skip_diff_check.sh
bash tests/scripts/lineno_fidelity_diff_check.sh
for t in trap_integration trap_pseudo_signals_integration funcname functions_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done
```
Note: `debug_firing_points`/`extdebug_skip`/`lineno_fidelity` fragments that put DEBUG inside a FUNCTION without `-T` would now (correctly) not fire — if any such fragment exists and breaks, it encoded the old behavior; fix the fragment to add `set -T` (matching bash) or move it top-level.

- [ ] **Step 6: dbg-support diff shrinkage + dbg-support2 PASS**
```bash
ulimit -v 2000000
HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=90 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 200 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "dbg-support2 \|"   # PASS
HUCK_BASH_TEST_CATEGORY=dbg-support HUCK_TEST_TIMEOUT=120 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 240 bash tests/bash-test-suite/runner.sh > /tmp/dbgs.md 2>&1
SC=$(grep -oE "/tmp/huck-bash-tests[^ ]*" /tmp/dbgs.md | head -1)
echo "dbg-support diff: $(wc -l < $SC/dbg-support.diff) lines (was 1171)"
```
Record the new diff size (expect a large drop) and the residual classes for the sub-arc's next iterations. dbg-support2 MUST stay PASS.

- [ ] **Step 7: Full sweep**
```bash
cargo build --release -p huck
ulimit -v 1500000; timeout 550 bash tests/scripts/run_diff_checks.sh   # green (coproc flake pre-existing)
```

- [ ] **Step 8: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/traps.rs tests/scripts/functrace_diff_check.sh
git commit -m "$(cat <<'EOF'
v327: gate DEBUG/RETURN traps on functrace inside subroutines (#272)

bash inherits the DEBUG and RETURN traps into a function or sourced script
only under `set -T` (functrace); huck fired them unconditionally. Gate both in
traps.rs: DEBUG in fire_debug_trap (in a Function/Source frame && !functrace ->
don't fire), RETURN in fire_return_trap (fire only under functrace). The
dominant fix for the dbg-support bash-suite category. Top-level firing
unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** the two gates + harness + dbg-support measurement + regressions are all Task 1 (spec §Design, §Testing).
- **Placeholders:** none; both gates are complete code. The sourced-file harness case notes a tempfile helper (as `lineno_fidelity_diff_check.sh` does).
- **Type consistency:** `in_subroutine` via `FrameKind::{Function,Source}`; `shell.shell_options.functrace: bool`; `fire_debug_trap -> DebugDecision`.
- **Scope:** functrace gate only; caller/`$LINENO`/command-sub are the sub-arc's later iterations.
