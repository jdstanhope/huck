# v326 — extdebug: honor DEBUG skip/return at compound-header fire sites

Issue: [#262](https://github.com/jdstanhope/huck/issues/262)

## Problem

v324 added DEBUG-trap fires at compound headers (`for`/`select`/`case`/
arith-for), and v325 gave them the right `$LINENO`. But those fires **ignore**
the `DebugDecision` returned by `fire_debug_trap` (`let _ = …`): under
`shopt -s extdebug`, a non-zero DEBUG-action status should skip the pending
unit and status 2 in a subroutine should simulate `return`. Body simple
commands already honor this (v322); the compound-header fires do not.

## bash 5.2.21 semantics (verified; driven by the DEBUG action's exit status)

| fire site | non-zero DEBUG status (`SkipCommand`) | status 2 in a function/source (`ReturnFromSub(2)`) |
|---|---|---|
| **for / select header** (per iteration) | skip the loop **from here** — the current iteration's body and all remaining iterations do not run | return from the subroutine |
| **case header** (once, at entry) | skip the whole `case` | return from the subroutine |
| **arith-for init** | skip evaluating the init expression (loop still runs, var uninitialized) | return |
| **arith-for cond** | exit the loop (as if the condition were false) | return |
| **arith-for step** | skip evaluating the step expression (loop continues, var unchanged) | return |

(`$BASH_COMMAND` is not needed — the decision is status-driven; huck's
`fire_debug_trap` already computes it.) Body simple commands inside these
constructs keep honoring the decision through `run_exec_single_inner` (v322).

## Design

At each compound-header fire site in `crates/huck-engine/src/executor.rs`,
replace `let _ = crate::traps::fire_debug_trap(shell);` (the fire is already
preceded by the v325 `current_lineno` header-line stamp) with a match that
honors the decision. `ReturnFromSub(n)` is uniform everywhere:
`return ExecOutcome::FunctionReturn(n);` — the existing signal `call_function`
converts to a function return. `SkipCommand` is per-site.

### `run_for_inner` / `run_select_inner` (per-iteration header fire)

The fire is at the top of the iteration loop (before `try_set`/the body).
```rust
match crate::traps::fire_debug_trap(shell) {
    DebugDecision::Proceed => {}
    DebugDecision::SkipCommand => break,          // skip this iteration + the rest of the loop
    DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
}
```
`break` exits the `for value in values` loop, and the function returns its
accumulated `last` (`ExecOutcome::Continue(...)`), so `$?` is the loop's status
so far (0 if skipped on the first iteration) — matching bash's "skip the loop
from here."

### `run_case_inner` (entry fire, once)

The fire is before the subject expansion / pattern loop.
```rust
match crate::traps::fire_debug_trap(shell) {
    DebugDecision::Proceed => {}
    DebugDecision::SkipCommand => return ExecOutcome::Continue(shell.last_status()),
    DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
}
```

### `run_arith_for_inner` (init / cond / step fires)

Three fires, three different `SkipCommand` effects:

- **init fire** (before `eval_arith_word(init)`): `SkipCommand` → skip the init
  evaluation but keep looping. Gate the init eval on a `run_init` flag:
  ```rust
  let mut run_init = true;
  match crate::traps::fire_debug_trap(shell) {
      DebugDecision::Proceed => {}
      DebugDecision::SkipCommand => run_init = false,
      DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
  }
  if run_init && let Some(init) = &clause.init { /* existing eval_arith_word(init) */ }
  ```
- **cond fire** (before the cond eval, each iteration): `SkipCommand` → exit the
  loop (`break`), as if the condition were false:
  ```rust
  match crate::traps::fire_debug_trap(shell) {
      DebugDecision::Proceed => {}
      DebugDecision::SkipCommand => break,
      DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
  }
  ```
- **step fire** (before the step eval, each iteration): `SkipCommand` → skip the
  step evaluation, keep looping. Gate the step eval on a per-iteration flag
  (same shape as init).

The existing v325 header-line stamp (`if clause.line != 0 { current_lineno =
line_base() + clause.line; }`) stays immediately before each fire.

### Notes

- `fire_debug_trap`'s recursion guard (v323) means the DEBUG action's own
  commands don't re-fire, so no special handling.
- Without `extdebug`, or with a zero DEBUG status, every site gets `Proceed`
  and behaves exactly as today (no behavior change — the firing counts and
  `$LINENO` are untouched).
- **Pipeline-stage** skip/return is explicitly out of scope → [#268](https://github.com/jdstanhope/huck/issues/268)
  (forked stages + fd plumbing). Those fires keep `let _ = …`.

## Testing

Gate = bash 5.2.21 fidelity.

1. **Bash-diff harness** `tests/scripts/extdebug_skip_diff_check.sh` (model on
   `trap_zero_diff_check.sh`). The DEBUG action is a helper function returning a
   controlled status (a bare `return` in the action is a top-level error), e.g.
   `tr() { [[ <cond> ]] && return 1; return 0; }; trap tr DEBUG`. Cases (compare
   byte-identical, incl. exit):
   - for-header skip on iter 1 (whole loop skipped) and on iter 2 (remaining
     skipped); for-body skip (one iteration, v322 — regression guard).
   - select-header skip (via `<<< 1`).
   - case-header skip (whole case skipped); case-body skip (v322 guard).
   - arith-for: init skip (loop runs uninitialized), cond skip (loop exits),
     step skip (loop continues, var unchanged — bound with a safety `break`).
   - ReturnFromSub: a header fire returning 2 inside a function → the function
     returns 2, abandoning the construct (`f; echo $?`).
   - no-extdebug: a non-zero DEBUG status does NOT skip any construct
     (regression guard); zero-status DEBUG proceeds.
2. **Regression**: `dbg-support2` stays PASS; the DEBUG firing-count harness
   (`debug_firing_points_diff_check.sh`) and `lineno_fidelity_diff_check.sh`
   stay green (Proceed path unchanged); `trap_integration` /
   `trap_pseudo_signals_integration` green; full `run_diff_checks.sh` sweep
   green; huck-engine lib green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard sweeps with `ulimit -v 1500000` + `timeout`; run the
`-p huck` trap integration binaries single-threaded before push; NO GPL bash
text.

## Scope

**In scope.** Honoring `SkipCommand`/`ReturnFromSub` at the for/select/case
header and arith-for init/cond/step fire sites. The harness + regressions.

**Out of scope.** Pipeline-stage skip/return (#268). No change to the fires'
counts or `$LINENO` (v324/v325). No change to the body-command decision path
(v322). `$BASH_COMMAND` (unimplemented, unrelated).

## Documentation

- Removes a divergence (no new intentional one); #262 auto-closes via the PR
  body (`Closes #262`).
