# huck v36 — `trap` Pseudo-Signals ERR / DEBUG / RETURN Design

**Goal:** Close M-22 by adding the three remaining bash pseudo-signals
to v35's `trap` builtin:

- **ERR** — fires after any command's non-zero exit, with bash 5.x
  exemptions (`if`/`elif`/`while`/`until` conditions, `!` negation, LHS
  of `||` chain).
- **DEBUG** — fires before each simple command, anywhere (inside or
  outside functions).
- **RETURN** — fires after a function returns, with the function's
  positional args still in scope and `$?` set to the return status.

**Why:** v35 marked M-22 as `[partial]` because EXIT and 13 real signals
shipped but the other three pseudo-signals were deferred. v36 closes
the gap; M-22 flips to `[fixed v36]`.

## Forms

The `trap` builtin syntax is unchanged from v35. Three new accepted
signal names:

| Name | Variant |
|---|---|
| `EXIT` | `TrapSignal::Exit` (v35) |
| `ERR` | `TrapSignal::Err` (v36) |
| `DEBUG` | `TrapSignal::Debug` (v36) |
| `RETURN` | `TrapSignal::Return` (v36) |
| (real signal names) | `TrapSignal::Real(n)` (v35) |

`trap ACTION ERR`, `trap ACTION DEBUG`, `trap ACTION RETURN`, `trap -
ERR` (reset), `trap "" ERR` (ignore) all work identically to their EXIT
counterparts. `trap -p` lists all four pseudo-signals (in order EXIT,
ERR, DEBUG, RETURN) before any real-signal traps.

## Semantics

### ERR

Fires when a command's non-zero exit is the "final" status for its
enclosing list — matching bash 5.x's rules for `set -e` exit
candidates.

**Fires when** ALL of the following hold:
- The command's exit status is non-zero.
- The command is not inside an `if`/`elif`/`while`/`until` condition
  (the `Shell::err_suppressed_depth` counter is 0 in that scope).
- The command's connector in `execute_sequence_body` is NOT `Or`
  (i.e. failure isn't "handled" by a following `|| cmd2` clause).

> **Note on `!` negation:** bash 5.x also exempts `!`-negated
> pipelines from ERR. Huck currently does NOT parse `!` as pipeline
> negation — `! cmd` is treated as a literal command (exit 127
> "command not found"). This is a pre-existing gap unrelated to v36;
> documented separately. The ERR exemption for `!` is naturally moot
> in huck until `!` negation is implemented.

**Worked examples** (`trap 'echo CAUGHT' ERR` set):

| Script | Fires? |
|---|---|
| `false` | yes |
| `false && true` | yes — cmd1 fails, no Or-handler |
| `true && false` | yes — cmd2 fails, no further connector |
| `false \|\| true` | no — cmd1's failure handled by `\|\|` |
| `false \|\| false` | yes — cmd2 fails, no further connector |
| `if false; then :; fi` | no — inside if-condition |
| `while false; do :; done` | no — inside while-condition |
| `! cmd` (any) | n/a — `!` not supported in huck (parses as `command not found`) |

### DEBUG

Fires before each simple command — anywhere, including inside function
bodies. Compound commands (`if`/`while`/`for`/`case`/`{ }`) do NOT
themselves fire DEBUG; only the simple commands inside them do.

**Fires when**:
- The executor enters `run_exec_single` (the single dispatch point for
  every simple command).
- The recursion guard isn't set for DEBUG (the action running inside a
  DEBUG trap doesn't re-fire DEBUG for the action's own commands).

**Worked examples**:

| Script | DEBUG fire count |
|---|---|
| `trap 'echo DBG' DEBUG; true` | 1 (the `true`) |
| `trap 'echo DBG' DEBUG; if true; then true; fi` | 2 (condition's `true` + body's `true`) |
| `trap 'echo DBG' DEBUG; f() { true; }; f` | 1 (the `true` inside `f`) — note: the simple commands invoked by the action itself (`echo DBG`) are recursion-suppressed |

### RETURN

Fires when a function returns — explicit `return`, falling off the
end, or via an inner `Exit`/loop break propagating out.

**Fires when**:
- `call_function` is about to restore the caller's positional args.
- Specifically: BETWEEN `run_command(&body, ...)` and the
  `shell.positional_args = saved` line.
- `$?` is set to the function's return status BEFORE the action runs,
  so `trap 'echo $?' RETURN; f() { return 7; }; f` prints `7`.

The action runs WITH the function's `$0`, `$1`, …, `$@` still in
scope. After the action returns, the caller's positional args are
restored as usual.

## Recursion guard

`Shell::firing_trap: Option<TrapSignal>` tracks which pseudo-trap is
currently mid-action.

- `fire_pseudo_trap` (the shared body for fire_err / fire_debug /
  fire_return) returns immediately if `shell.firing_trap == Some(sig)`.
- Different signals do NOT cross-suppress: if a DEBUG action triggers
  ERR (via a failing command), ERR fires normally.
- EXIT is NOT routed through this guard — it self-removes via
  `traps.remove`, so re-firing is impossible by construction.

## ERR exemption depth

`Shell::err_suppressed_depth: u32` — non-zero means ERR is suppressed
in the current dynamic scope.

**Increment/decrement sites:**
- `run_if` — around the condition evaluation. Each `elif` condition
  gets the same push/pop.
- `run_while` and `run_until` — around the condition evaluation.
- **Negated pipeline** — NOT applicable in v36. Huck doesn't currently
  support `!` as pipeline negation (treats `!` as a literal command).
  When `!`-negation is added in a future iteration, the implementer
  will need to add push/pop around the negated pipeline's execution.

Symmetric push/pop guarantees: on entry to the suppressing region,
increment; on every exit path (normal return, error, propagation),
decrement. Use a guard struct (`struct ErrSuppressGuard<'a>(&'a mut
Shell);` with `Drop`) to ensure decrement on early returns.

## Storage extension

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrapSignal {
    Exit,
    Err,
    Debug,
    Return,
    Real(i32),
}
```

`parse_trap_signal` adds three new arms:

```rust
if name == "EXIT" { return Ok(TrapSignal::Exit); }
if name == "ERR" { return Ok(TrapSignal::Err); }
if name == "DEBUG" { return Ok(TrapSignal::Debug); }
if name == "RETURN" { return Ok(TrapSignal::Return); }
// ... existing numeric + SIG-prefix paths unchanged ...
```

`install` and `reset` extend the existing match so the four
pseudo-signals all use the simple `shell.traps.insert(sig, action)` /
`shell.traps.remove(&sig)` paths:

```rust
match sig {
    TrapSignal::Exit | TrapSignal::Err | TrapSignal::Debug | TrapSignal::Return => {
        shell.traps.insert(sig, action);
        Ok(())
    }
    TrapSignal::Real(signum) => { /* existing OS-handler path */ }
}
```

`print_active_traps` sort key in `src/builtins.rs`:

| TrapSignal | Sort key | Display name |
|---|---|---|
| `Exit` | 0 | `EXIT` |
| `Err` | 1 | `ERR` |
| `Debug` | 2 | `DEBUG` |
| `Return` | 3 | `RETURN` |
| `Real(n)` | `100 + n` | (table lookup) |

## Fire helpers

Three new public functions in `src/traps.rs`:

```rust
pub fn fire_err_trap(shell: &mut Shell) {
    fire_pseudo_trap(shell, TrapSignal::Err);
}

pub fn fire_debug_trap(shell: &mut Shell) {
    fire_pseudo_trap(shell, TrapSignal::Debug);
}

pub fn fire_return_trap(shell: &mut Shell) {
    fire_pseudo_trap(shell, TrapSignal::Return);
}
```

Shared body:

```rust
fn fire_pseudo_trap(shell: &mut Shell, sig: TrapSignal) {
    if shell.firing_trap == Some(sig) {
        return;
    }
    let action = match shell.traps.get(&sig) {
        Some(Some(text)) => text.clone(),
        _ => return,
    };
    let prev = shell.firing_trap.replace(sig);
    let _ = crate::shell::process_line(&action, shell);
    shell.firing_trap = prev;
}
```

The `prev` save-and-restore handles cross-signal firing (e.g. DEBUG
action triggers ERR via a failing command; ERR fires; on ERR exit,
DEBUG's guard is restored).

## Executor hook points

### DEBUG hook

`src/executor.rs::run_exec_single` (line 1363) — add as the FIRST line
of the function body:

```rust
fn run_exec_single(cmd: &ExecCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    crate::traps::fire_debug_trap(shell);
    // ... existing body ...
}
```

This single site covers every simple command (external, builtin,
function call). Compound commands dispatch through other functions and
naturally don't fire DEBUG.

### RETURN hook

`src/executor.rs::call_function` (line 1344). Insert BETWEEN
`run_command(&body, …)` and `shell.positional_args = saved`. Also set
`$?` to the function's status before firing so the action sees it:

```rust
let result = run_command(&body, shell, sink);
let status_for_trap = match &result {
    ExecOutcome::FunctionReturn(n) => *n,
    ExecOutcome::Continue(c) => *c,
    _ => shell.last_status(),  // Exit/LoopBreak/LoopContinue: keep prev status
};
shell.set_last_status(status_for_trap);
crate::traps::fire_return_trap(shell);
shell.function_arg0.pop();
shell.positional_args = saved;
match result {
    ExecOutcome::FunctionReturn(n) => ExecOutcome::Continue(n),
    other => other,
}
```

### ERR hook

`src/executor.rs::execute_sequence_body` (line 55) — in both
`set_last_status(c)` blocks (one after `seq.first`, one in the `for
(connector, command) in &seq.rest` loop), AFTER the v34 fatal-PE check
and the v35 `dispatch_pending_traps` poll, add the ERR firing logic.

The "next connector is Or" check differs per block:
- First block (after `seq.first`): peek at `seq.rest.first()`.
- Loop block: peek at the NEXT iteration's connector — use an indexed
  loop, or refactor to a windowed pair iteration.

Sketch for the first block:

```rust
if let ExecOutcome::Continue(c) = status {
    shell.set_last_status(c);
    if shell.pending_fatal_pe_error.is_some() {
        return ExecOutcome::Continue(c);
    }
    crate::traps::dispatch_pending_traps(shell);
    let next_is_or = matches!(seq.rest.first(), Some((Connector::Or, _)));
    if c != 0 && shell.err_suppressed_depth == 0 && !next_is_or {
        crate::traps::fire_err_trap(shell);
    }
}
```

For the loop block, track each iteration's index `i`; `next_is_or` is
`matches!(seq.rest.get(i + 1), Some((Connector::Or, _)))`.

### ERR-suppression push/pop sites

Three call sites in `src/executor.rs`:

**`run_if`** — around each condition evaluation (the initial `if` and
each `elif`):

```rust
shell.err_suppressed_depth += 1;
let cond_status = execute_sequence_body(&condition, shell, sink);
shell.err_suppressed_depth -= 1;
```

(Or use a guard struct for panic-safety; see note in the "ERR exemption
depth" section.)

**`run_while`** and **`run_until`** — around the condition. Each loop
iteration re-evaluates the condition; push/pop wraps that call.

**Negated pipeline** — locate huck's pipeline-AST handling for `!`. If
`Pipeline { negated: bool }` exists, push around the body when negated.
If not, locate where `!` is parsed and stashed and push/pop around the
appropriate executor call.

## Subshell behavior

`clear_for_subshell` already clears `shell.traps`. v36 adds two more
resets:

```rust
pub fn clear_for_subshell(shell: &mut Shell) {
    // ... existing SigId unregistration + traps.clear + trap_pending reset ...
    shell.firing_trap = None;
    shell.err_suppressed_depth = 0;
}
```

The four pseudo-signals naturally don't inherit because they're in the
cleared traps map.

## Error handling

| Condition | Behavior |
|---|---|
| `trap action FOO` where FOO is unknown | Existing v35 error: "FOO: invalid signal specification", `$?=1`. Unchanged. |
| Action body has syntax error | Detected at fire time via `process_line`; error printed by parser; trap dispatch continues. Same as v35. |
| Recursive trap (same signal) | Suppressed by `firing_trap` guard. Action runs once; no infinite recursion. |
| Trap action exits the shell (`trap 'exit' ERR; false`) | Action calls `exit`; `process_line` returns `ExecOutcome::Exit(c)` which the outer caller propagates. Shell exits normally (firing EXIT trap along the way). |

## Testing

### Module unit tests (~8 new in `src/traps.rs`)

- `parse_trap_signal_err` — `"ERR"` → `Ok(Err)`.
- `parse_trap_signal_debug` — `"DEBUG"` → `Ok(Debug)`.
- `parse_trap_signal_return` — `"RETURN"` → `Ok(Return)`.
- `fire_err_trap_runs_action_without_remove` — register ERR; fire; assert action ran AND trap entry remains.
- `fire_err_trap_recursion_guard_suppresses_reentry` — set `firing_trap = Some(Err)`; fire ERR; assert action did NOT run.
- `fire_debug_trap_recursion_guard_suppresses_reentry` — same shape for Debug.
- `fire_return_trap_recursion_guard_suppresses_reentry` — same shape for Return.
- `clear_for_subshell_resets_firing_trap_and_err_depth` — pre-set both fields; call clear; assert both are reset.

### Builtin unit tests (~4 new in `src/builtins.rs`)

- `trap_err_pseudo_signal_registers` — `trap 'echo X' ERR` puts the entry in `shell.traps`.
- `trap_debug_pseudo_signal_registers` — same for DEBUG.
- `trap_return_pseudo_signal_registers` — same for RETURN.
- `trap_p_lists_pseudo_signals_in_order` — set EXIT/ERR/DEBUG/RETURN, assert `trap -p` output lists them in that exact order.

### Integration tests (~14 new in `tests/trap_pseudo_signals_integration.rs`)

DEBUG (4):
- `debug_fires_before_simple_command`
- `debug_fires_inside_function_body`
- `debug_does_not_fire_for_compound_command_itself`
- `debug_recursion_guard_prevents_infinite_fire`

ERR (7 — `! cmd` test omitted; see note below):
- `err_fires_on_simple_command_failure`
- `err_does_not_fire_in_if_condition`
- `err_does_not_fire_in_while_condition`
- `err_does_not_fire_on_or_chain_lhs`
- `err_fires_on_or_chain_when_all_fail`
- `err_fires_on_and_chain_lhs_failure`
- `err_fires_on_and_chain_rhs_failure`

(Bash's ERR exemption for `! cmd` is moot in huck — `!` parses as a
literal command, not pipeline negation. No test for that case.)

RETURN (2):
- `return_fires_after_function_return`
- `return_action_sees_function_status` — `trap 'echo got $?' RETURN; f() { return 7; }; f; echo done=$?` → `got 7` then `done=7`.

**Test isolation**: each test spawns a fresh `huck` via
`Command::new(huck_binary())` (same harness as v35's trap_integration
tests). State doesn't leak.

**Total new tests**: ~25 (4 traps + 4 builtin + 4 DEBUG + 7 ERR + 2
RETURN + 4 module/recursion). Baseline goes from 1372 → ~1397.

## Documentation

- `docs/bash-divergences.md`:
  - **M-22** flips from `[partial v35]` to `[fixed v36]`.
  - Update description: all four pseudo-signals (EXIT/ERR/DEBUG/RETURN)
    + 13 real signals now supported.
  - REMOVE the "Out of scope" clause about ERR/DEBUG/RETURN being
    deferred — they're now in scope.
  - KEEP the M-41 (limited signal set) and `trap '' SIG`-not-SIG_IGN
    notes — both unchanged from v35.
  - ADD a new "Out of scope" item: `$BASH_COMMAND` variable inside
    DEBUG/ERR/RETURN actions is not set (action runs but the variable
    expands to empty).
- Changelog row.
- README v36 row.

## Scope (in)

- ERR / DEBUG / RETURN pseudo-signals via `trap ACTION ERR`, `trap -p
  DEBUG`, etc.
- bash 5.x ERR exemptions: `if`/`elif`/`while`/`until` conditions and
  LHS of `||` chain. (`!` negation is also a bash exemption but is
  moot in huck — see scope-out.)
- DEBUG fires for every simple command (top-level + inside function
  bodies).
- RETURN fires after function returns with the function's positional
  args still in scope and `$?` set to the return status.
- Recursion guard via `Shell::firing_trap` — same signal doesn't
  re-fire from within its own action; different signals still fire.
- Subshell trap-clear extended to reset `firing_trap` +
  `err_suppressed_depth`.

## Scope (out)

- **`$BASH_COMMAND` variable** — would require capturing command text
  at each fire site and threading through Shell. Deferred.
- **DEBUG for compound commands** — matches bash default (without `set
  -T`/`-o functrace`). Adding compound coverage would need a `set` flag
  which itself is M-08 (deferred).
- **`set -T` / `set -o functrace`** — M-08 still deferred.
- **`.` / `source` RETURN firing** — huck has no source builtin
  (separate gap).
- **`trap '' SIG` true SIG_IGN** — v35 known limitation; unchanged.
- **`$FUNCNAME` inside RETURN action** — bash provides FUNCNAME stack;
  huck has function_arg0 but it's not exposed as a variable. Deferred.
- **`! cmd` pipeline negation** — pre-existing huck gap (unrelated to
  v36). Huck parses `!` as a literal command, exit 127. When `!`
  negation lands in a future iteration, the v36 ERR exemption depth
  push/pop will need to be wired into that pipeline's dispatch.

## Implementation tasks (handoff to writing-plans)

| # | Task | Notes |
|---|---|---|
| 1 | `TrapSignal::{Err,Debug,Return}` + parse_trap_signal extensions + install/reset arm + sort key in print_active_traps + ~4 traps + ~4 builtin unit tests | Pure data extension. |
| 2 | `Shell::firing_trap` + `Shell::err_suppressed_depth` + fire_err/debug/return helpers + shared fire_pseudo_trap + clear_for_subshell update + recursion-guard unit tests | Storage + fire mechanism. |
| 3 | DEBUG hook (`run_exec_single`) + RETURN hook (`call_function` with $?-set-before-fire) | Two single-line insertions + the $?-handling pattern. |
| 4 | ERR hook in execute_sequence_body (both blocks, with next-connector-Or peek) + err_suppressed_depth push/pop in run_if/run_while/run_until (negated pipeline n/a — huck doesn't parse `!`) | Most complex task. Touches multiple sites. |
| 5 | Integration tests + docs (M-22 → fixed v36; changelog; README) + full-suite verify | Coverage + close-out. |

Process: subagent-driven per `[[huck-iteration-workflow]]` on
`v36-trap-pseudo-signals` branch. Final code-reviewer over the whole
branch diff before `merge --no-ff` into `main`.
