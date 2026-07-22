# v322 — DEBUG trap under extdebug Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fire the DEBUG trap before bare assignments, make `$LINENO` in the DEBUG action equal the pending command's line, and implement the `extdebug` non-zero-skips-command / status-2-in-subroutine-simulates-return semantics — flipping the bash-suite `dbg-support2` category FAIL → PASS.

**Architecture:** A pure decision helper + a rewritten `fire_debug_trap` (returns a `DebugDecision`, reframes `$LINENO`) in `traps.rs`; both leaf simple-command dispatch sites in `executor.rs` honor the decision; an `extdebug()` accessor in `shell_state.rs`. Reuses existing machinery: the v315 `eval_frame`/`line_base()` line-reframe and `ExecOutcome::FunctionReturn` for the in-subroutine return.

**Tech Stack:** Rust; huck-engine crate; bash-diff `*_diff_check.sh` harness.

Spec: `docs/superpowers/specs/2026-07-22-dbg-support2-debug-trap-design.md`
Issue: [#255](https://github.com/jdstanhope/huck/issues/255)

## Global Constraints

- bash 5.2.21 fidelity. Verified DEBUG/extdebug semantics (extdebug ON): a
  DEBUG action exiting **non-zero** skips the pending command; status **exactly
  2** with a live subroutine frame (function or sourced) simulates `return 2`
  from it; extdebug **off** ignores a non-zero status. The action's top-level
  `$LINENO` equals the pending command's line. DEBUG must fire before bare
  assignments (`x=1`) as well as regular commands.
- Do NOT change: the `fire_pseudo_trap` path for `RETURN`/`ERR`/real signals;
  the already-correct per-command DEBUG firing for regular commands; the inner
  function-body `$LINENO` (`FUNCNAME[1]`) behavior.
- Commit trailer on every commit:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit.
- Per repo memory: build the binary with `cargo build -p huck`; run tests
  per-crate single-threaded (`cargo test -p huck-engine --lib --jobs 1 --
  --test-threads 1`); guard bash-diff sweeps with `ulimit -v 1500000` +
  `timeout`. NEVER `cargo test --workspace` (OOMs this box). Run the `-p huck`
  trap/lineno integration binaries single-threaded before pushing. NEVER copy
  bash's GPL `dbg-support2.tests` text into the repo — author synthetic
  fragments.

---

### Task 1: `DebugDecision` + pure decision helper + `fire_debug_trap` rewrite + `extdebug()`

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (add `extdebug()` accessor near `extglob()`, ~line 1226)
- Modify: `crates/huck-engine/src/traps.rs` (`DebugDecision`, `debug_decision`, `fire_debug_trap` rewrite; update 2 existing tests; add new tests)
- Test: same `traps.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `pub enum DebugDecision { Proceed, SkipCommand, ReturnFromSub(i32) }`;
  `pub fn fire_debug_trap(shell: &mut Shell) -> DebugDecision`;
  `pub fn Shell::extdebug(&self) -> bool`.
- Consumes: existing `Shell` fields `firing_trap`, `traps`, `eval_frame`,
  `current_lineno`, `last_status()`, `call_stack`, `source_depth`,
  `shopt_options`; `crate::shell::process_line`.
- Note: the existing call site `crate::traps::fire_debug_trap(shell);`
  (executor.rs ~4257, statement form) keeps compiling — the returned value is
  discarded there until Task 2 wires it. Behavior is unchanged after Task 1
  (the old site ignores the decision), so Task 1 lands independently.

- [ ] **Step 1: Add the `extdebug()` accessor**

In `shell_state.rs`, next to `extglob()` (~line 1226):

```rust
/// True when `shopt -s extdebug` is in effect.
pub fn extdebug(&self) -> bool {
    self.shopt_options.get("extdebug").unwrap_or(false)
}
```

- [ ] **Step 2: Write the failing decision-matrix + LINENO tests**

Add to `traps.rs` tests. The pure `debug_decision` does not exist yet, and
`fire_debug_trap` still returns `()`, so these fail to compile → that is the
red state.

```rust
#[test]
fn debug_decision_matrix() {
    use DebugDecision::*;
    // extdebug off: never skips, whatever the status.
    assert_eq!(debug_decision(false, 0, false), Proceed);
    assert_eq!(debug_decision(false, 1, false), Proceed);
    assert_eq!(debug_decision(false, 2, true), Proceed);
    // extdebug on, status 0: proceed.
    assert_eq!(debug_decision(true, 0, false), Proceed);
    // extdebug on, non-zero non-2: skip one command (top level or in sub).
    assert_eq!(debug_decision(true, 1, false), SkipCommand);
    assert_eq!(debug_decision(true, 3, true), SkipCommand);
    // extdebug on, status 2, NOT in a subroutine: skip (can't return).
    assert_eq!(debug_decision(true, 2, false), SkipCommand);
    // extdebug on, status 2, IN a subroutine: simulate return 2.
    assert_eq!(debug_decision(true, 2, true), ReturnFromSub(2));
}

#[test]
fn fire_debug_trap_reframes_lineno_to_pending_command_line() {
    let mut shell = Shell::new();
    shell.current_lineno = 5;
    shell
        .traps
        .insert(TrapSignal::Debug, Some("probe=$LINENO".to_string()));
    let d = fire_debug_trap(&mut shell);
    assert_eq!(d, DebugDecision::Proceed); // no extdebug, action exits 0
    // The action's top-level $LINENO reflected the pending command's line (5),
    // not 1 (which is what a fresh process_line would otherwise stamp).
    assert_eq!(shell.get("probe"), Some("5"));
}

#[test]
fn fire_debug_trap_skips_when_extdebug_and_action_nonzero() {
    let mut shell = Shell::new();
    shell.shopt_options.set("extdebug", true);
    shell
        .traps
        .insert(TrapSignal::Debug, Some("false".to_string())); // exits 1
    assert_eq!(fire_debug_trap(&mut shell), DebugDecision::SkipCommand);
}
```

- [ ] **Step 3: Run the tests to verify they fail (compile error)**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1 debug_decision fire_debug_trap`
Expected: compile error (unknown `debug_decision` / return-type mismatch) — the red state.

- [ ] **Step 4: Implement `DebugDecision`, `debug_decision`, and the `fire_debug_trap` rewrite**

In `traps.rs`, add the enum (near the top, pub):

```rust
/// What a command-dispatch site must do after the DEBUG trap action ran.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DebugDecision {
    /// Run the pending command normally.
    Proceed,
    /// extdebug + non-zero DEBUG status: skip the pending command.
    SkipCommand,
    /// extdebug + status 2 in a subroutine: simulate `return n`.
    ReturnFromSub(i32),
}

/// Pure mapping from (extdebug, DEBUG action exit status, in-a-subroutine) to
/// the post-DEBUG action. Kept pure for exhaustive unit testing; the effects
/// (skipping/returning) live at the executor dispatch sites.
pub(crate) fn debug_decision(extdebug: bool, status: i32, in_subroutine: bool) -> DebugDecision {
    if !extdebug || status == 0 {
        return DebugDecision::Proceed;
    }
    if status == 2 && in_subroutine {
        return DebugDecision::ReturnFromSub(2);
    }
    DebugDecision::SkipCommand
}
```

Rewrite `fire_debug_trap` (dedicated, no longer via `fire_pseudo_trap`):

```rust
/// Fires the DEBUG pseudo-signal trap before a command runs. Returns the
/// action the dispatch site must take (Proceed / SkipCommand / ReturnFromSub).
/// Recursion-guarded (the action's own commands don't re-fire). While the
/// action runs, `$LINENO` is reframed so its top-level line equals the pending
/// command's line (`current_lineno`, stamped by the caller just before).
pub fn fire_debug_trap(shell: &mut Shell) -> DebugDecision {
    if shell.firing_trap == Some(TrapSignal::Debug) {
        return DebugDecision::Proceed;
    }
    let action = match shell.traps.get(&TrapSignal::Debug) {
        Some(Some(text)) => text.clone(),
        _ => return DebugDecision::Proceed,
    };
    // Reframe $LINENO: with line_base() = eval_frame.saturating_sub(1) and each
    // command stamping current_lineno = line_base() + cmd.line, setting
    // eval_frame = Some(current_lineno) makes the action's line-1 command
    // resolve back to current_lineno. Restore afterward. (Skip for the
    // synthesized line-0 case, where there is no meaningful pending line.)
    let prev_frame = shell.eval_frame;
    if shell.current_lineno != 0 {
        shell.eval_frame = Some(shell.current_lineno);
    }
    let prev_firing = shell.firing_trap.replace(TrapSignal::Debug);
    let _ = crate::shell::process_line(&action, shell, false);
    shell.firing_trap = prev_firing;
    shell.eval_frame = prev_frame;

    let in_subroutine = !shell.call_stack.is_empty() || shell.source_depth > 0;
    debug_decision(shell.extdebug(), shell.last_status(), in_subroutine)
}
```

- [ ] **Step 5: Update the two existing `fire_debug_trap` tests**

Both now assert the returned `DebugDecision` (a `Shell::new()` has no extdebug
and the actions exit 0, so both are `Proceed`), keeping their original checks:

```rust
#[test]
fn fire_debug_trap_runs_action_without_remove() {
    let mut shell = Shell::new();
    shell
        .traps
        .insert(TrapSignal::Debug, Some("FOO=dbg_ran".to_string()));
    assert_eq!(fire_debug_trap(&mut shell), DebugDecision::Proceed);
    assert_eq!(shell.get("FOO"), Some("dbg_ran"));
    assert!(shell.traps.contains_key(&TrapSignal::Debug));
}

#[test]
fn fire_debug_trap_recursion_guard_suppresses_reentry() {
    let mut shell = Shell::new();
    shell.firing_trap = Some(TrapSignal::Debug);
    shell
        .traps
        .insert(TrapSignal::Debug, Some("FOO=should_not_run".to_string()));
    assert_eq!(fire_debug_trap(&mut shell), DebugDecision::Proceed);
    assert_eq!(shell.get("FOO"), None);
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1 debug_decision fire_debug_trap`
Expected: all PASS (the matrix, the two reframed originals, the LINENO reframe, the skip).

- [ ] **Step 7: Full huck-engine lib suite (regression guard) + fmt**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1`
Expected: green. Then `cargo fmt --all`.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/traps.rs
git commit -m "$(cat <<'EOF'
v322: fire_debug_trap returns a DebugDecision + reframes $LINENO (#255)

Add DebugDecision (Proceed/SkipCommand/ReturnFromSub) + a pure debug_decision
helper (extdebug + action status + in-subroutine -> action), rewrite
fire_debug_trap to reframe $LINENO to the pending command's line (eval_frame)
and return the decision, and add Shell::extdebug(). The existing call site
ignores the return until the executor is wired (Task 2), so behavior is
unchanged here.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Honor the decision at both dispatch sites + harness + baseline flip

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (the `fire_debug_trap` call in `run_exec_single_inner` ~line 4257; the `SimpleCommand::Assign` arm ~line 3865)
- Create: `tests/scripts/debug_trap_extdebug_diff_check.sh`
- Modify: `docs/bash-test-suite-baseline.md` (PASS 18 → 19, FAIL 64 → 63; `dbg-support2` → PASS; fix the stale root-cause note at ~:150)

**Interfaces:**
- Consumes Task 1: `crate::traps::{fire_debug_trap, DebugDecision}`.
- Produces: end-to-end DEBUG/extdebug behavior; an auto-discovered
  `*_diff_check.sh`.

- [ ] **Step 1: Wire the Exec path**

In `run_exec_single_inner`, replace the bare statement
`crate::traps::fire_debug_trap(shell);` (~line 4257) with:

```rust
match crate::traps::fire_debug_trap(shell) {
    crate::traps::DebugDecision::Proceed => {}
    crate::traps::DebugDecision::SkipCommand => {
        return ExecOutcome::Continue(shell.last_status());
    }
    crate::traps::DebugDecision::ReturnFromSub(n) => {
        return ExecOutcome::FunctionReturn(n);
    }
}
```

This early-returns before any argument expansion, so a skipped command has no
side effects. Nothing between the LINENO stamp and this call needs unwinding
(only `builtin_usage_error = None`, the LINENO stamp, and the `procsub_base`
snapshot — no procsubs are pushed on a skip).

- [ ] **Step 2: Wire the Assign path**

In the `SimpleCommand::Assign(items, line)` arm (~line 3865), after the
existing `$LINENO` stamp and before `run_assignment_list`, branch on the fire:

```rust
SimpleCommand::Assign(items, line) => {
    if *line != 0 {
        shell.current_lineno = shell.line_base() + *line;
    }
    match crate::traps::fire_debug_trap(shell) {
        crate::traps::DebugDecision::Proceed => {
            let procsub_base = shell.procsub_pending.len();
            let st = run_assignment_list(items, shell, sink, err_sink);
            drain_procsubs(shell, procsub_base);
            ExecOutcome::Continue(st)
        }
        crate::traps::DebugDecision::SkipCommand => {
            ExecOutcome::Continue(shell.last_status())
        }
        crate::traps::DebugDecision::ReturnFromSub(n) => {
            ExecOutcome::FunctionReturn(n)
        }
    }
}
```

Keep the existing v318 procsub-drain comment above this arm. On a skip the RHS
is never expanded, so no procsub is realized and no drain is needed.

- [ ] **Step 3: Build + quick manual parity check against bash**

```bash
cargo build -p huck
```
Then eyeball the three headline scenarios (bash vs `./target/debug/huck`):
- `n=0; trap 'n=$((n+1))' DEBUG; x=1; y=2; echo $n` → both `3`.
- The `dbg-support2` shape (fire per command, `$LINENO` arg tracks the line,
  `return 2` at top level skips `x=2`).
- extdebug + return-2 inside a function → the function unwinds (returns 2).

- [ ] **Step 4: Write the bash-diff harness**

First read an existing small `tests/scripts/*_diff_check.sh` (e.g.
`trap_zero_diff_check.sh`) and copy its structure exactly (shebang, `set -u`,
`HUCK_BIN` default+check, `check "label" 'frag'` comparing
`bash --norc --noprofile` vs `$HUCK_BIN` with `2>&1; echo "EXIT:$?"`,
byte-identical, PASS/FAIL counters, `exit $((FAIL>0?1:0))`).

Create `tests/scripts/debug_trap_extdebug_diff_check.sh` with synthetic
fragments (NOT bash's GPL `dbg-support2.tests`). Cover:

```sh
check "fires before assignments" 'n=0; trap '\''n=$((n+1))'\'' DEBUG; x=1; y=2; echo $n'
check "lineno tracks pending cmd" 'trap '\''echo L=$LINENO'\'' DEBUG
echo one
echo two'
check "funcname main from action" 'trap '\''echo ${FUNCNAME[1]:-none}-${FUNCNAME[0]:-none}'\'' DEBUG; :'
check "extdebug top-level skip (rc2)" 'shopt -s extdebug; de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 2; fi; return 0; }; trap '\''tr'\'' DEBUG; x=1; de=2; x=2; echo x=$x'
check "extdebug top-level skip (rc1)" 'shopt -s extdebug; de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 1; fi; return 0; }; trap '\''tr'\'' DEBUG; x=1; de=2; x=2; echo x=$x'
check "extdebug func return (rc2)" 'shopt -s extdebug; de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 2; fi; return 0; }; trap '\''tr'\'' DEBUG; f(){ echo A; de=2; echo B; echo C; }; f; echo "ret=$?"'
check "extdebug func skip-one (rc1)" 'shopt -s extdebug; de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 1; fi; return 0; }; trap '\''tr'\'' DEBUG; f(){ echo A; de=2; echo B; echo C; }; f; echo "ret=$?"'
check "no extdebug: no skip" 'de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 2; fi; return 0; }; trap '\''tr'\'' DEBUG; x=1; de=2; x=2; echo x=$x'
```

> Quoting these multi-line/nested-single-quote fragments in the harness is
> fiddly. The gate is bash==huck parity for the emitted construct — if a
> fragment is hard to express, verify what you actually wrote runs the intended
> program (compare bash vs huck) rather than matching this literal text; adjust
> the fragment, not the shells. Every `check` must PASS.

Build both binaries and run:
```bash
cargo build -p huck && cargo build --release -p huck
ulimit -v 1500000
timeout 120 bash tests/scripts/debug_trap_extdebug_diff_check.sh
```
Expected: every check PASS.

- [ ] **Step 5: Full bash-diff sweep (regression guard)**

```bash
ulimit -v 1500000
timeout 300 bash tests/scripts/run_diff_checks.sh
```
Expected: all green (new check auto-discovered). Note any pre-existing
unrelated failure; STOP + report BLOCKED if the v322 change caused one.

- [ ] **Step 6: Confirm the `dbg-support2` category flips to PASS**

With bash 5.2.21 source (`BASH_SOURCE_DIR`):
```bash
ulimit -v 2000000
HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=60 \
  BASH_SOURCE_DIR=<path-to-bash-5.2.21> \
  timeout 150 bash tests/bash-test-suite/runner.sh
```
Expected: `dbg-support2 | PASS`, empty diff.

- [ ] **Step 7: Update the baseline doc**

In `docs/bash-test-suite-baseline.md`: bump PASS 18 → 19, FAIL 64 → 63; move
`dbg-support2` to PASS; replace its stale FAIL note (currently blames "LINENO
inside functions", the wrong root) with a one-line PASS note (v322 #255).
Adjust the near-miss ranking (drop `dbg-support2`). Do NOT paste bash output.

- [ ] **Step 8: Run the `-p huck` trap/lineno integration binaries**

```bash
ulimit -v 2500000
for t in trap_integration bash_source_lineno lineno script_line_numbers_integration eval_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done
```
Expected: all green (guards $LINENO/eval/trap behavior the reframe touches).

- [ ] **Step 9: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/debug_trap_extdebug_diff_check.sh docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v322: honor DebugDecision at dispatch sites; flip dbg-support2 (#255)

Both leaf simple-command dispatch sites (Exec + bare Assign) fire the DEBUG
trap and honor the returned decision: SkipCommand -> skip the command;
ReturnFromSub(n) -> FunctionReturn(n). Adds debug_trap_extdebug_diff_check.sh
and records the dbg-support2 category flip FAIL->PASS (bash-suite PASS 18->19).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** Task 1 = `extdebug()` + `DebugDecision` + `fire_debug_trap`
  (LINENO reframe #2 + decision #3) + unit tests (spec §Design, §Testing 2).
  Task 2 = both dispatch sites (#1 assignment fire + #3 skip/return honoring) +
  harness + category flip + baseline (spec §Design dispatch, §Testing 1/3/4,
  §Documentation).
- **Placeholders:** none; every code block is complete. The harness fragment
  quoting caveat is a known fiddliness with an explicit fallback (parity is the
  gate), not a placeholder.
- **Type consistency:** `fire_debug_trap -> DebugDecision`, `debug_decision`
  signature, and `ExecOutcome::{Continue, FunctionReturn}` match across tasks;
  `extdebug()`/`last_status()`/`line_base()`/`eval_frame`/`call_stack`/
  `source_depth` are the verified existing names.
- **Scope:** value already-correct paths (regular per-command firing, inner
  function `$LINENO`, RETURN/ERR traps) untouched; only DEBUG + the two
  dispatch sites change.
