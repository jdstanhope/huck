# v328 — RETURN trap does not fire while a DEBUG trap action is executing

Issue: [#273](https://github.com/jdstanhope/huck/issues/273) — second step of the `dbg-support` bash-suite category sub-arc.

## Problem

When a shell function (or sourced script) returns **while the DEBUG trap
action is executing**, huck fires the RETURN trap; bash 5.2.21 suppresses the
RETURN trap for the duration of the DEBUG trap action.

```sh
set -T
helper(){ return 0; }
trap 'helper' DEBUG          # the DEBUG action calls a function
trap 'echo RET' RETURN
echo cmd
# bash: cmd        huck: RET cmd   (huck fires RETURN when helper returns)
```

This is the dominant residual class in the `dbg-support` bash-suite diff after
v327 — the test's DEBUG action is `print_debug_trap $LINENO`, a function call,
so every DEBUG fire spuriously emits a `return lineno: 1 print_debug_trap`
line (~430 of the ~1171 diff lines).

### Precisely which trap suppresses RETURN (verified against bash 5.2.21)

| context | RETURN fires? |
|---|---|
| function returns while a **DEBUG** action runs | **no** — suppressed |
| function returns while a **RETURN** action runs | no — same-signal (already guarded, v323) |
| function returns while an **ERR** action runs | **yes** — `RET inerr` |

So RETURN is suppressed specifically while the DEBUG trap is on the firing
stack (bash's `run_return_trap` skips when the DEBUG trap is executing). It is
suppressed regardless of whether the returning function is the whole action or
one command of a multi-command action (`trap 'helper; :' DEBUG` → still
suppressed). DEBUG and ERR are NOT suppressed while other traps run (v323).

## Design

huck already tracks the set of currently-firing pseudo-signals in
`shell.firing_traps` (v323). `fire_return_trap` (`crates/huck-engine/src/traps.rs`)
already gates on functrace/extdebug (v327) and delegates to `fire_pseudo_trap`,
whose same-signal guard covers RETURN-during-RETURN. Add one more guard —
suppress RETURN while a DEBUG action is firing:

```rust
pub fn fire_return_trap(shell: &mut Shell) {
    if !(shell.shell_options.functrace || shell.extdebug()) {
        return;
    }
    // #273: RETURN does not fire for a function/source that returns while the
    // DEBUG trap action is executing (bash suppresses the RETURN trap for the
    // duration of the DEBUG trap). ERR does NOT suppress RETURN; the
    // same-signal (RETURN-during-RETURN) case is already covered by
    // fire_pseudo_trap's firing_traps guard.
    if shell.firing_traps.contains(&TrapSignal::Debug) {
        return;
    }
    fire_pseudo_trap(shell, TrapSignal::Return);
}
```

`fire_return_trap` is called only from `call_function` (a function return). A
function called from within a DEBUG action returns with `firing_traps ==
[Debug]` (or `[..., Debug, ...]`), so `contains(Debug)` suppresses exactly the
spurious RETURN. A real function that returns outside any trap action has an
empty (or Debug-free) `firing_traps`, so its RETURN still fires.

## Testing

Gate = bash 5.2.21 fidelity + `dbg-support` diff shrinkage.

1. **Bash-diff harness** `tests/scripts/return_in_trap_action_diff_check.sh`
   (model on `trap_zero_diff_check.sh`). Cases (byte-identical incl. exit):
   - RETURN suppressed while a DEBUG action calls a function (the core case,
     single- and multi-command DEBUG action).
   - RETURN still fires while an ERR action calls a function (`RET inerr`).
   - a real function return (outside any trap action) still fires RETURN under
     `-T`/`extdebug`.
   - the `print_debug_trap`-shape: a DEBUG action = a function call, with a
     RETURN trap installed → no spurious RETURN per DEBUG fire.
   - top-level / no-functrace cases unchanged (regression guard from v327).
2. **`dbg-support` diff shrinkage**: re-run `HUCK_BASH_TEST_CATEGORY=dbg-support`
   and record the new diff size (measured: 1171 → 635 — the ~430
   `return lineno` lines drop to 32). Note the residual (`caller`, `$LINENO`,
   #274 entry-fire) for the sub-arc's next iterations.
3. **Regression**: `dbg-support2` stays PASS; the DEBUG firing-count /
   extdebug-skip / lineno-fidelity / functrace harnesses stay green;
   `trap_integration` / `trap_pseudo_signals_integration` green (a RETURN trap
   whose action itself calls a function under functrace now correctly does not
   double-fire — update any test that asserted the old behavior); full
   `run_diff_checks.sh` sweep green; huck-engine lib green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard sweeps with `ulimit -v 1500000` + `timeout`; run the
`-p huck` trap integration binaries single-threaded before push; NO GPL bash
text.

## Scope

**In scope.** The one `firing_traps.contains(Debug)` guard in
`fire_return_trap`; the harness; the `dbg-support` measurement; regressions.

**Out of scope (later sub-arc iterations).** #274 (DEBUG on function entry);
the `caller` builtin; `$LINENO`-in-trap fidelity; DEBUG suppression inside trap
actions (bash does NOT suppress DEBUG during ERR/RETURN actions — huck already
matches, verified).

## Documentation

- Removes a divergence (no new intentional one). #273 auto-closes via the PR
  body (`Closes #273`).
