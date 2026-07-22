# v324 — DEBUG trap firing points: align with bash across pipelines, loops, case, and functrace subshells

Issue: [#257](https://github.com/jdstanhope/huck/issues/257)

## Problem

huck fires the DEBUG trap before top-level simple commands and bare
assignments (v322), and inside `while`/`until`/`if`/`{ }` groups (their body
simple commands route through the simple-command path). But bash fires DEBUG
at several points huck misses. Verified against bash 5.2.21 (`trap 'echo
DBG:[$BASH_COMMAND]' DEBUG`):

| construct | bash fires DEBUG before… | huck today |
|---|---|---|
| **pipeline** | **each stage** (`echo a`, then `cat`) | neither stage |
| **`for` loop** | the header **once per iteration** + the body | body only |
| **arithmetic `for`** | **each** init / cond / update expression + body | body only |
| **`case`** | the header (`case a in`) + the matched body | body only |
| **`select`** | the header once per menu iteration + body | body only |
| **subshell under `set -T`** | the (functrace-inherited) commands inside | trap cleared in the fork → nothing |

`while`/`until`/`if`/`{ }`/plain subshell already match and are unchanged.

This is DEBUG **firing-point/count** fidelity. Two related concerns are
explicitly **out of scope** (own issues):
- **`$LINENO` of the new fires** — [#261](https://github.com/jdstanhope/huck/issues/261).
  The compound clauses carry no source line, and there is a pre-existing
  compound-body line bug. v324's new fires report whatever `current_lineno`
  holds; correctness of that line is #261.
- **extdebug skip/return at the new sites** — the new fires run the DEBUG
  action but treat the result as `Proceed` (see "Decision handling" below).
  The existing simple-command sites still honor skip/return for the body
  commands.

## Design

Every new fire uses the existing `crate::traps::fire_debug_trap(shell)` (the
v322/v323 entry point: recursion-guarded via the `firing_traps` set, reframes
`$LINENO`, returns a `DebugDecision`).

### Decision handling at the new sites

The primary goal is firing-count fidelity. The new fires call
`fire_debug_trap` and **ignore** the returned `DebugDecision` (bind `let _ =`)
— i.e. they always proceed. Rationale: honoring `SkipCommand`/`ReturnFromSub`
at a pipeline stage / loop-iteration header / case header is an extdebug edge
case with its own per-site "what does skip mean here" semantics; deferring it
keeps v324 to the firing points. Body simple commands inside these constructs
still route through `run_exec_single_inner`, which honors the decision. A
follow-up issue tracks honoring skip/return at compound/pipeline fire sites.

### 1. Pipeline stages (`spawn_pipeline`, executor.rs ~6316)

bash fires DEBUG in the **parent**, before forking each stage (the DEBUG
action's output goes to the parent's stdout/terminal, not into the pipe — an
in-child fire would send it down the pipe). Multi-stage pipelines go through
`run_multi_stage` → `spawn_pipeline`, which forks each stage. Fire
`fire_debug_trap` in the parent immediately before forking each stage, in
stage order. Single-stage pipelines run in-process via `run_command` and
already fire through the normal path — unchanged.

### 2. `for` loop (`run_for_inner`, executor.rs ~1814)

bash fires DEBUG before the loop header **once per iteration**. In the
`for value in values` loop, fire `fire_debug_trap` at the top of each
iteration (alongside the existing per-iteration xtrace block, before
`try_set(var, value)` / the body).

### 3. Arithmetic `for` (`run_arith_for_inner`, executor.rs ~2005)

bash fires DEBUG before the **init** expression (once), before **each cond**
evaluation, and before **each update** expression (plus the body, which
already fires). Fire `fire_debug_trap` at each of those three evaluation
points. `init`/`cond`/`step` are `Option<Word>`; fire before evaluating a
present expression (skip the fire when the expression is absent, matching
bash — an empty section is not a command).

### 4. `case` (`run_case_inner`, executor.rs ~2337)

bash fires DEBUG once before the case header (before pattern matching). Fire
`fire_debug_trap` once at entry to `run_case_inner`, before the
subject-word expansion / pattern loop.

### 5. `select` (`run_select_inner`, executor.rs ~2138)

Like `for`: bash fires the header before each menu iteration (each time it
reads a selection and runs the body). Fire `fire_debug_trap` at the top of
each select iteration (mirror the `for` placement — before the body runs).

### 6. functrace subshells (`clear_for_subshell` call site, executor.rs ~8215)

A subshell fork calls `crate::traps::clear_for_subshell(shell)`, which clears
ALL traps — so DEBUG/RETURN never fire inside a `( )`. bash, under `set -T`
(`functrace`), inherits DEBUG and RETURN into subshells. Gate the clear: when
`shell.shell_options.functrace` is set, preserve the DEBUG and RETURN trap
actions across the subshell reset (clear everything else as today). Then the
subshell's body simple commands fire DEBUG through the normal path.

Minimal, targeted change: keep DEBUG+RETURN entries in `shell.traps` when
functrace is on. Do NOT touch the (separate, pre-existing) question of whether
DEBUG-in-a-function should be gated on functrace — huck already fires DEBUG in
functions; that divergence is not part of #257.

## Testing

Gate = bash 5.2.21 fidelity for firing **counts** (not `$LINENO`, per scope).

1. **Bash-diff harness.** Add `tests/scripts/debug_firing_points_diff_check.sh`
   (model on `trap_zero_diff_check.sh`). Because a subshell's DEBUG counter
   increments are lost when the subshell exits, and to avoid `$LINENO`
   (deferred), the DEBUG action **prints a fixed marker** so fires are counted
   from stdout, byte-identical to bash. Cases (each a fragment run through both
   shells, comparing full output):
   - pipeline: `trap 'echo D' DEBUG; echo a | cat` → bash's `D`/`a` sequence.
   - `for`: `trap 'echo D' DEBUG; for x in 1 2; do echo $x; done`.
   - arith-for: `trap 'echo D' DEBUG; for ((i=0;i<2;i++)); do echo $i; done`.
   - `case`: `trap 'echo D' DEBUG; case a in a) echo m;; esac`.
   - `select`: driven with a piped choice, or asserted via a small fixed menu
     (keep it deterministic; if `select` interaction is awkward in the harness,
     assert its header-fire via a `<<<` selection).
   - functrace subshell: `set -T; trap 'echo D' DEBUG; ( echo a; echo b )` →
     bash fires D inside; without `set -T`, `( echo a; echo b )` fires D only
     for the subshell command in the parent (regression guard).
   - regressions: `while`/`until`/`if`/`{ }` fire counts unchanged; a lone
     top-level command still fires once.
   The harness's DEBUG action must not itself fail (no ERR-trap interplay) and
   must not print `$LINENO`.
2. **Unit/behavioral checks** where cheap: a `for`-loop fire count via a
   counter in the SAME shell (not a subshell) — e.g. `n` incremented by the
   DEBUG action equals bash's count. (Subshell cases must use the stdout-marker
   form, since a subshell counter is lost on exit.)
3. **Regression guards.** `dbg-support2` category stays PASS (v322 semantics
   unaffected — the new fires are additional points, and the DEBUG decision
   path is unchanged); `trap_integration` /
   `trap_pseudo_signals_integration` green; full `run_diff_checks.sh` sweep
   green; huck-engine + huck-syntax lib suites green. Confirm no double-fire
   regressions (e.g. a single-stage pipeline still fires exactly once).

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard sweeps with `ulimit -v 1500000` + `timeout`; run the
`-p huck` trap integration binaries single-threaded before push.

## Scope

**In scope.** The six firing-point additions above (pipeline stages, `for`
per-iteration, arith-for per-expression, `case` header, `select` per-iteration,
functrace-subshell trap preservation); the diff harness.

**Out of scope (own issues).** `$LINENO` fidelity of the new fires (#261);
honoring extdebug skip/return at the new fire sites (new follow-up); DEBUG
`$BASH_COMMAND` (unimplemented, separate); DEBUG-in-function functrace gating
(pre-existing, separate). None of these block #257's firing-count fidelity.

## Documentation

- `docs/architecture.md`: if trap firing is described, note DEBUG now fires at
  compound headers / per loop iteration / per pipeline stage, and is inherited
  into subshells under `functrace`.
- Removes a divergence (no new intentional one); #257 auto-closes via the PR
  body (`Closes #257`).
