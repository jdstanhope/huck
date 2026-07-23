# v327 — functrace gating: DEBUG/RETURN traps fire inside functions/sourced files only under `set -T`

Issue: [#272](https://github.com/jdstanhope/huck/issues/272) — the first step of the `dbg-support` bash-suite category sub-arc.

## Problem

bash's `set -T` / `set -o functrace`: the DEBUG and RETURN traps are inherited
into shell functions and sourced scripts only when functrace is set; otherwise
they are NOT. huck fires both **unconditionally** — inside functions and
sourced files even without `-T`. This is the dominant divergence in the
`dbg-support` bash-suite category (1171-line diff: ~430 RETURN-trap lines +
~312 DEBUG-trap lines), because `dbg-support.tests` toggles `set -T`/`set +T`
throughout and huck over-fires in every `-T`-off section.

Verified against bash 5.2.21:

```sh
f(){ echo body; }
trap 'echo RET' RETURN
f            # bash: body        huck: body RET   (huck over-fires RETURN)
set -T
f            # bash: body RET    huck: body RET

g(){ echo g; }; f(){ g; echo f; }
trap 'echo D' DEBUG
f            # bash (no -T): D g f     huck: D D g D f   (huck fires DEBUG inside f/g)
set -T; f    # bash: D D g D … D f …   (fires inside; both match under -T)

trap 'echo D' DEBUG; source ./x   # bash: no DEBUG inside x; huck: fires inside x
```

## Design

Gate the two pseudo-trap fires on functrace + whether we are inside a
subroutine (a function or sourced file). The predicate is exactly the v322
`in_subroutine` (a `FrameKind::Function` or `FrameKind::Source` on the call
stack — the main script's base frame is neither). At the top level
(`in_subroutine == false`) the traps always fire, as today.

### `fire_debug_trap` (`crates/huck-engine/src/traps.rs`)

Hoist the existing `in_subroutine` computation (currently at ~line 155, used
for the `ReturnFromSub` decision) to the top of the function, and add the gate
right after the recursion guard, BEFORE the action lookup/run:

```rust
pub fn fire_debug_trap(shell: &mut Shell) -> DebugDecision {
    if shell.firing_traps.contains(&TrapSignal::Debug) {
        return DebugDecision::Proceed;
    }
    let in_subroutine = shell.call_stack.iter().any(|f| {
        matches!(f.kind, crate::shell_state::FrameKind::Function
                       | crate::shell_state::FrameKind::Source)
    });
    // functrace (`set -T`): DEBUG is inherited into a function or sourced
    // script only under functrace; at the top level it always fires. (bash)
    if in_subroutine && !shell.shell_options.functrace {
        return DebugDecision::Proceed;
    }
    let action = match shell.traps.get(&TrapSignal::Debug) { … };
    … (eval_frame reframe, $? save, push/run/pop, unchanged) …
    let decision = debug_decision(shell.extdebug(), shell.last_status(), in_subroutine);
    …
}
```

This centralizes the gate: every DEBUG fire site (body commands via
`run_exec_single_inner`, the compound-header fires, the assign path) inherits
it automatically. A gated fire returns `Proceed` (no action runs, the command
proceeds normally).

### `fire_return_trap` (`crates/huck-engine/src/traps.rs`)

`fire_return_trap` is called only from `call_function` (executor.rs ~4089), at
a function return — always inside a Function frame. Gate it on functrace:

```rust
pub fn fire_return_trap(shell: &mut Shell) {
    // RETURN is inherited into a function/sourced script only under
    // functrace (`set -T`); without it the trap does not fire on return. (bash)
    if !shell.shell_options.functrace {
        return;
    }
    fire_pseudo_trap(shell, TrapSignal::Return);
}
```

### Notes / scope of the gate

- **Fire-time vs entry-time.** bash inherits the traps at function *entry*
  based on functrace *then*; this design checks functrace at *fire* time. These
  agree unless a function toggles functrace mid-body (rare; the `dbg-support`
  test toggles only at top level). Entry-time capture is a possible follow-up.
- **Command substitutions.** functrace also gates DEBUG/RETURN in command
  substitutions. huck's `$(…)` may not push a Function/Source frame, so this
  gate might not cover it — verified/handled as a residual (follow-up if it
  shows in the `dbg-support` diff after this fix).
- **Subshells** are already handled (v324: a subshell fork preserves DEBUG/
  RETURN across `clear_for_subshell` only under functrace).
- The extdebug decision path (v322/v326) is unchanged — a gated DEBUG simply
  doesn't fire, so no skip/return happens inside an ungated-out function.

## Testing

Gate = bash 5.2.21 fidelity + `dbg-support` diff shrinkage.

1. **Bash-diff harness** `tests/scripts/functrace_diff_check.sh` (model on
   `trap_zero_diff_check.sh`). Cases (compare byte-identical incl. exit):
   - DEBUG inside a function: fires only under `-T` (count via `echo D`).
   - RETURN on function return: fires only under `-T`.
   - nested functions (`f` calls `g`): DEBUG/RETURN inside both only under `-T`.
   - sourced file: DEBUG inside it only under `-T`; RETURN on source return
     under `-T`.
   - top-level DEBUG/RETURN unaffected (fire regardless of `-T`).
   - `set -T; …; set +T; …` toggling between calls (the `dbg-support` shape).
2. **`dbg-support` diff shrinkage** (not necessarily a flip yet): re-run
   `HUCK_BASH_TEST_CATEGORY=dbg-support` and record the new diff size — it
   should drop dramatically from 1171 lines (the RETURN/DEBUG over-fire classes
   collapse). Note the residual classes (`caller`, `$LINENO`) for the next
   sub-arc iterations.
3. **Regression**: `dbg-support2` category stays PASS (its DEBUG fires are
   top-level, `in_subroutine == false` — unaffected; its RETURN behavior:
   confirm); the DEBUG firing-count harness (`debug_firing_points_diff_check.sh`),
   `extdebug_skip_diff_check.sh`, and `lineno_fidelity_diff_check.sh` stay green
   (their DEBUG fires are top-level unless a fragment is in a function — check
   each); `trap_integration` / `trap_pseudo_signals_integration` green (update
   any test that asserted the old always-fire-in-function behavior — those were
   wrong vs bash); full `run_diff_checks.sh` sweep green; huck-engine lib green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard sweeps with `ulimit -v 1500000` + `timeout`; run the
`-p huck` trap integration binaries single-threaded before push; NO GPL bash
text.

## Scope

**In scope.** The functrace gate in `fire_debug_trap` and `fire_return_trap`;
the harness; the `dbg-support` diff-shrinkage measurement; regressions.

**Out of scope (later sub-arc iterations).** The `caller` builtin (~25 diff
lines); `$LINENO`-in-trap fidelity (~54 lines); command-substitution functrace
gating (residual); entry-time-vs-fire-time functrace capture. These are the
next steps toward a `dbg-support` flip.

## Documentation

- `docs/architecture.md`: if trap firing is described, note DEBUG/RETURN are
  gated on functrace inside functions/sourced files.
- Removes a divergence (no new intentional one). A `dbg-support` sub-arc
  tracking issue is opened; the PR references it.
