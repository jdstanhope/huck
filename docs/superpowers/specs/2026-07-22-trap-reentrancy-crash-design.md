# v323 — Cross-signal trap re-entrancy: fix the DEBUG↔ERR/RETURN stack-overflow crash

Issue: [#256](https://github.com/jdstanhope/huck/issues/256)

## Problem

When two pseudo-signal traps whose actions each run a command mutually
trigger each other, huck recurses until the Rust stack overflows (SIGABRT):

```sh
trap false ERR; trap false DEBUG; echo x
```
- bash 5.2.21: prints `x`, rc 0.
- huck: `stack overflow, aborting` (SIGABRT, rc 134).

## Root cause

`Shell.firing_trap: Option<TrapSignal>` is a **single slot**. The fire
helpers (`fire_pseudo_trap` for ERR/RETURN, `fire_debug_trap` for DEBUG)
save the previous value in a local and restore it afterward — so the Rust
call stack *is* a stack of active trap signals — but the recursion guard
only inspects the **current top**: `firing_trap == Some(sig)`.

Trace of the repro:
1. `echo x` → DEBUG fires. `firing_trap = Some(Debug)`. Action `false`.
2. `false` fails → ERR fires. Guard: `Some(Debug) != Some(Err)` → runs.
   `firing_trap` is **overwritten** to `Some(Err)` (Debug survives only as a
   local in `fire_debug_trap`'s frame). Action `false`.
3. `false` fails → tries ERR: `Some(Err) == Some(Err)` → suppressed. But
   before it runs, DEBUG fires: `Some(Err) != Some(Debug)` → **runs again**
   (Debug is still on the stack, but the single slot forgot it). Action
   `false` → fails → ERR fires (`Some(Debug) != Some(Err)`) → …

DEBUG and ERR alternate; neither same-signal guard ever catches the
alternation because the slot only remembers the innermost signal. Unbounded
recursion → stack overflow.

bash bounds this: a trap never re-enters **itself**, but *different* traps
may nest. In the repro bash runs DEBUG(`false`)→ERR(`false`)→both now
suppressed→unwinds→`echo x` prints `x`. Verified: bash also bounds a
three-way DEBUG+ERR+RETURN chain (each fires, then all suppressed).

## Design

Replace the single slot with a **stack of currently-active trap signals** and
make the guard test membership, not equality:

### Field (shell_state.rs)

```rust
// was: pub firing_trap: Option<crate::traps::TrapSignal>,
pub firing_traps: Vec<crate::traps::TrapSignal>,
```
Initialized to `Vec::new()`. `TrapSignal` is `Copy + PartialEq + Eq`, and at
most three pseudo-signals (DEBUG/ERR/RETURN) are ever active at once, so the
vec is tiny; it doubles as the existing save/restore stack.

### Guard + push/pop (traps.rs)

`fire_pseudo_trap` (ERR/RETURN):
```rust
fn fire_pseudo_trap(shell: &mut Shell, sig: TrapSignal) {
    if shell.firing_traps.contains(&sig) {
        return; // already running THIS trap up the stack — suppress re-entry
    }
    let action = match shell.traps.get(&sig) {
        Some(Some(text)) => text.clone(),
        _ => return,
    };
    shell.firing_traps.push(sig);
    let _ = crate::shell::process_line(&action, shell, false);
    shell.firing_traps.pop();
}
```

`fire_debug_trap` (DEBUG) — same guard, keeping the v322 `$LINENO` reframe,
`$?` save/restore, and `DebugDecision` return:
```rust
pub fn fire_debug_trap(shell: &mut Shell) -> DebugDecision {
    if shell.firing_traps.contains(&TrapSignal::Debug) {
        return DebugDecision::Proceed;
    }
    let action = match shell.traps.get(&TrapSignal::Debug) {
        Some(Some(text)) => text.clone(),
        _ => return DebugDecision::Proceed,
    };
    // ... eval_frame reframe (unchanged) ...
    let saved_status = shell.last_status();
    shell.firing_traps.push(TrapSignal::Debug);
    let _ = crate::shell::process_line(&action, shell, false);
    shell.firing_traps.pop();
    // ... restore eval_frame (unchanged) ...
    let in_subroutine = /* unchanged Function|Source predicate */;
    let decision = debug_decision(shell.extdebug(), shell.last_status(), in_subroutine);
    shell.set_last_status(saved_status);
    decision
}
```
(The `firing_traps.push`/`pop` replaces the old `firing_trap.replace(sig)` /
`= prev` save-restore; the vec push/pop *is* the save/restore.)

### `clear_for_subshell` (traps.rs)

```rust
// was: shell.firing_trap = None;
shell.firing_traps.clear();
```

### Why this is correct and bounded

- A signal already on `firing_traps` is suppressed → a trap never re-enters
  itself (matches bash), which is exactly what the repro needs (ERR does not
  re-fire while an outer ERR is active; DEBUG does not re-fire while an outer
  DEBUG is active).
- Different signals still nest (DEBUG may fire during an ERR action and vice
  versa) — matches bash.
- The set of pseudo-signals is finite (3), so the active chain is bounded to
  depth ≤ 3; no runaway recursion is possible.

No behavior change for the common single-trap paths: with one trap active,
`contains` behaves exactly like the old `== Some(sig)` check. The v322 DEBUG
decision/`$LINENO`/`$?` logic is untouched apart from the push/pop swap.

## Testing

Gate = bash 5.2.21 fidelity + no crash.

1. **Bash-diff harness.** Add `tests/scripts/trap_reentrancy_diff_check.sh`
   (model on `trap_zero_diff_check.sh`): synthetic fragments through
   `bash --norc --noprofile` vs `$HUCK_BIN`, byte-identical incl. exit. Cases:
   - the crash repro: `trap false ERR; trap false DEBUG; echo x` → `x`, rc 0
     (huck must not abort).
   - DEBUG action fails with an ERR trap installed: bounded counts match bash
     (e.g. `d=…; e=…` counters over a couple of commands).
   - DEBUG fires during an ERR action but ERR does not re-enter itself.
   - RETURN + DEBUG under `set -T`: no crash, output matches bash.
   - single-trap regressions: a lone DEBUG/ERR/RETURN trap fires exactly as
     before (per-command DEBUG count unchanged; ERR once per failure).
2. **Unit tests (traps.rs).** Update the existing recursion-guard tests to the
   `firing_traps` vec (they currently set `firing_trap = Some(sig)`; change to
   `firing_traps.push(sig)` and assert suppression). Add a **cross-signal**
   test: with `firing_traps = [Debug]`, firing ERR runs (different signal);
   with `firing_traps = [Debug, Err]`, firing Debug is suppressed
   (`Proceed`, action does not run) — the exact defect. Update
   `clear_for_subshell_resets_firing_trap_and_err_depth` to assert
   `firing_traps.is_empty()`.
3. **Regression guards.** `dbg-support2` category stays PASS (the v322 DEBUG
   semantics are unaffected); `trap_integration` /
   `trap_pseudo_signals_integration` green; full `run_diff_checks.sh` sweep
   green; huck-engine lib suite green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard sweeps with `ulimit -v 1500000` + `timeout`; run the
`-p huck` trap integration binaries single-threaded before push.

## Scope

**In scope.** The `firing_trap` → `firing_traps: Vec` change; the membership
guard in both fire helpers; `clear_for_subshell`; the unit-test updates; the
new diff harness.

**Out of scope.** The other pre-existing DEBUG gaps (#257 subshell/compound
firing granularity, #258 multi-line eval `$LINENO`). No depth backstop (the
membership guard bounds nesting precisely; the design decision was guard-only).

## Documentation

- `docs/architecture.md`: if traps are described, note that the recursion
  guard tracks the set of active pseudo-signals (a trap never re-enters
  itself; different traps may nest), not a single slot.
- Removes a divergence (no new intentional one); #256 auto-closes via the PR
  body (`Closes #256`).
