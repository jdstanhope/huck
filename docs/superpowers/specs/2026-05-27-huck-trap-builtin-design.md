# huck v35 — `trap` Builtin Design

**Goal:** Close M-22 (initial scope — EXIT pseudo-signal + real signal
traps) by implementing the POSIX `trap` builtin. Users can register
shell-script actions to fire on shell exit OR on receipt of any
trappable signal in huck's existing 15-name signal table.

**Why:** Listed as **high impact** in `docs/bash-divergences.md` Tier 2.
Real bash scripts rely on `trap 'cleanup' EXIT` for resource cleanup
and `trap 'echo interrupted' INT` for graceful Ctrl-C handling.
Currently huck has no `trap` builtin at all; signals follow huck's
built-in defaults with no user override path.

## Forms

| Syntax | Meaning |
|---|---|
| `trap ACTION SIGNAL...` | Register ACTION (re-parsed each fire) for the listed signals. |
| `trap "" SIGNAL...` | Ignore the listed signals (SIG_IGN). |
| `trap - SIGNAL...` | Reset to default disposition. |
| `trap` (no args) | Same as `trap -p` — list active traps. |
| `trap -p` | List ALL active traps. Format: `trap -- 'action' SIGNAL`. |
| `trap -p SIGNAL...` | List traps for the named signals only. |
| `trap -l` | List signal name/number pairs (EXIT excluded — not a real signal). |

## Signals supported

Real signals: reuses huck's existing 15-name `kill` table, minus the two
POSIX-uncatchable signals (`KILL`/`9`, `STOP`/`19`). Trappable set:

| Name | Number |
|---|---|
| HUP | 1 |
| INT | 2 |
| QUIT | 3 |
| USR1 | 10 |
| USR2 | 12 |
| PIPE | 13 |
| ALRM | 14 |
| TERM | 15 |
| CHLD | 17 |
| CONT | 18 |
| TSTP | 20 |
| TTIN | 21 |
| TTOU | 22 |
| WINCH | 28 |

Names parsed in three forms: `INT` (POSIX), `SIGINT` (bash convenience),
`2` (numeric). All three resolve to the same `TrapSignal::Real(2)`.

Pseudo-signal: **EXIT** only. ERR/DEBUG/RETURN are scope-out for v35.

## Semantics

- **Action body** is stored as a raw `String` and re-parsed via
  `crate::shell::process_line` each time the trap fires. Variable
  expansion happens at fire time (so `trap 'echo $count' EXIT` reads
  the latest `$count` value at exit).
- **EXIT trap** fires ONCE on any normal shell-exit path. Self-removing:
  `Shell::traps.remove(&TrapSignal::Exit)` happens before the action runs,
  so recursive `exit` from within the EXIT action does not re-fire.
- **Real-signal traps** fire from the main loop after a polling
  checkpoint detects a pending bit in the bitmask. Async-signal-safe
  signal handler sets the bit; the main loop drains and dispatches.
- **`$?` inside trap actions** reflects the exit status of the command
  that ran immediately before the trap fired (this matches bash —
  `trap 'echo $?' EXIT; false; exit` prints `1`).
- **Trap actions run in the current shell scope** (not a subshell).
  Variable assignments persist; `cd` persists.
- **Recursive traps** are not blocked at the action level — the signal
  handler is naturally serialised because the main loop runs to
  completion before polling again. A `trap 'kill -INT $$' INT` would
  re-fire on the next polling checkpoint, not from within the running
  action.

## Subshell behavior

After `fork_and_run_in_subshell` clones Shell for the child:

```rust
cloned.traps.clear();
cloned.trap_pending = Arc::new(AtomicU32::new(0));
```

- EXIT trap cleared (parent's fires once, when parent exits).
- All real-signal traps cleared (POSIX: "Trapped signals that are not
  being ignored are reset to their original values in a subshell").
- The inherited OS-level signal handlers stay installed but write to a
  bitmask that no main loop reads, so they're effectively inert. The
  child's main loop (if it has one — subshells re-enter `execute()`
  not `run()`) doesn't poll the bitmask.

## AST

No AST changes. `trap` is a builtin invoked through the existing
`run_builtin` dispatch.

## Storage

### New module: `src/traps.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrapSignal {
    Exit,
    Real(i32),
}

// Set at Shell::new() time; signal handlers read it without going
// through Shell.
static TRAP_PENDING: OnceLock<Arc<AtomicU32>> = OnceLock::new();

pub fn install(shell: &mut Shell, sig: TrapSignal, action: Option<String>);
pub fn reset(shell: &mut Shell, sig: TrapSignal);
pub fn drain_pending(shell: &mut Shell) -> Vec<i32>;
pub fn dispatch_pending_traps(shell: &mut Shell);
pub fn fire_exit_trap(shell: &mut Shell);
pub fn parse_trap_signal(name: &str) -> Result<TrapSignal, String>;
pub fn name_table() -> &'static [(&'static str, i32)];
pub fn clear_for_subshell(shell: &mut Shell);
```

### `Shell` fields (`src/shell_state.rs`)

```rust
pub traps: HashMap<TrapSignal, Option<String>>,
//   None     → ignore (SIG_IGN registered)
//   Some(s)  → action s
//   absent   → default disposition

pub trap_pending: Arc<AtomicU32>,
//   Bit N corresponds to signal number N.
//   Initialised in Shell::new(); cloned for `trap_pending`'s atomic
//   reference identity in subshells (replaced post-fork).
```

`Shell::new()` initializes both AND calls
`crate::traps::init_pending_bitmask(&shell.trap_pending)` which sets
the `TRAP_PENDING` `OnceLock` so the C handler can find the right Arc.

## Signal-handler installation

`install()` installs huck's shared C-ABI trap handler via
`libc::sigaction`:

```rust
extern "C" fn trap_handler(sig: i32) {
    if let Some(flag) = TRAP_PENDING.get() {
        if (0..32).contains(&sig) {
            flag.fetch_or(1u32 << (sig as u32), Ordering::SeqCst);
        }
    }
}
```

Async-signal-safe: only `OnceLock::get` (lock-free read) and
`AtomicU32::fetch_or` (lock-free RMW). No allocation, no locks.

### Coexistence with existing huck signal handlers

- **SIGINT**: huck already installs a handler (`install_sigint_handler`
  at `src/shell.rs:193`) that sets `sigint_flag`. When the user does
  `trap action INT`, huck's `install()` replaces the SIGINT handler
  with a thin wrapper that:
  1. Sets the trap bitmask bit.
  2. Also sets `sigint_flag` (preserves existing rustyline cleanup).
  
  When `trap - INT` runs, `reset()` restores the original
  `install_sigint_handler`-installed handler.

- **SIGCHLD**: similar layering. Job-control reaping continues to work;
  a user CHLD trap fires alongside the reaper.

- **All other signals**: huck has no installed handler; trap installs
  one. `reset` restores `SIG_DFL`.

### Ignored signals on shell entry

POSIX: "Signals ignored upon entry to the shell cannot be trapped or
reset." If `sigaction` returns `SIG_IGN` for a signal at startup,
`install()` rejects subsequent `trap` calls for it. (Edge case; bash
also enforces this. Cheap to implement via a one-shot check at first
install per signal, or scanned once at Shell::new() for the trappable
set.)

For v35 scope: implement the check at Shell::new() — scan trappable
signals once, record which start ignored, reject `trap` for those.

## Bitmask polling

`dispatch_pending_traps(shell)` is the only function that runs trap
actions for real signals. It's called at three checkpoints:

| Site | File | Reason |
|---|---|---|
| Top of REPL loop | `src/shell.rs::run()` before `read_logical_command` | Signals received between commands. |
| After each pipeline in a sequence | `src/executor.rs::execute_sequence_body` after `set_last_status(c)` | Long sequences should fire traps between commands. |
| After foreground-pipeline wait | `src/executor.rs::wait_pipeline_raw` after the wait loop | Ctrl-C during `sleep 60` should fire trap as soon as wait returns. |

Body:

```rust
pub fn dispatch_pending_traps(shell: &mut Shell) {
    for sig in drain_pending(shell) {
        let action = match shell.traps.get(&TrapSignal::Real(sig)) {
            Some(Some(text)) => text.clone(),
            Some(None) | None => continue,
        };
        let _ = crate::shell::process_line(&action, shell);
    }
}
```

The `ExecOutcome` returned by `process_line` is ignored: trap actions
can call `exit`, which returns `Exit(code)` and the outer caller
already handles that. For other outcomes (`Continue`, `LoopBreak`,
etc.) the trap action's effect is local — the surrounding command
proceeds.

## EXIT trap firing

The pseudo-signal never sets a bitmask bit. A dedicated helper fires
at every shell-exit path:

```rust
pub fn fire_exit_trap(shell: &mut Shell) {
    let action = match shell.traps.remove(&TrapSignal::Exit) {
        Some(Some(text)) => text,
        _ => return,
    };
    let _ = crate::shell::process_line(&action, shell);
}
```

Self-removing (uses `remove`, not `get`) so recursive `exit` from
within the action doesn't re-fire.

### Call sites in `src/shell.rs::run()`

| Existing code | Trap insertion |
|---|---|
| `ExecOutcome::Exit(code) => { shell.history.save(); return code; }` | Insert `fire_exit_trap(&mut shell);` before `history.save()`. |
| Fatal-PE drain branch `return fatal_status;` | Insert `fire_exit_trap` before `history.save()`. |
| `ReadResult::Eof => { shell.history.save(); return shell.last_status(); }` | Insert `fire_exit_trap` before `history.save()`. |
| `ReadResult::EofMidCommand => { … return 2; }` | Insert `fire_exit_trap` before `history.save()`. |
| `ReadResult::ReadError(msg) => { … return 1; }` | Insert `fire_exit_trap` before the return. |

All five sites get the same one-line insertion before the existing
exit logic. The `remove` semantics guarantee only the first site that
gets reached fires the action.

## Builtin syntax

### `BUILTIN_NAMES` + `is_special_builtin`

In `src/builtins.rs:18-22`, add `"trap"` to `BUILTIN_NAMES`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap",
];
```

In `is_special_builtin` at `src/builtins.rs:33-35`, add `"trap"`:

```rust
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name, "break" | "continue" | "exit" | "export" | "return" | "trap" | "unset")
}
```

(Per the comment at `src/builtins.rs:28-32` already anticipating this.)

### Dispatch arm

In `run_builtin` at `src/builtins.rs:46-71`:

```rust
"trap" => builtin_trap(args, out, shell),
```

### `builtin_trap` function

Lives in `src/builtins.rs` (alongside the other builtins). Parses
arguments into forms per the table in §Forms above, then delegates to
`crate::traps::install` / `crate::traps::reset` / printing helpers.

#### Output formats

**`trap -p` / `trap` (no args)**:

```
trap -- 'action text' SIGNAL
```

One line per active trap. Signals sorted by name (EXIT, HUP, INT, …)
or by number — bash sorts by number, we match that.

Ignored signals print as:

```
trap -- '' SIGNAL
```

**`trap -l`** (4-column format):

```
 1) HUP   2) INT   3) QUIT  10) USR1
12) USR2 13) PIPE 14) ALRM 15) TERM
17) CHLD 18) CONT 20) TSTP 21) TTIN
22) TTOU 28) WINCH
```

Fixed 4 columns × N rows. Right-pad signal numbers to 2 chars; left-pad
name to 5 chars after `)`.

## Error handling

| Condition | Behavior |
|---|---|
| Unknown signal name (e.g. `trap foo NOPE`) | `eprintln!("huck: trap: NOPE: invalid signal specification");` return Continue(1). |
| `trap foo KILL` / `trap foo STOP` | `eprintln!("huck: trap: KILL: cannot trap");` return Continue(1). |
| `trap foo SIG_IGN_AT_STARTUP` | `eprintln!("huck: trap: NAME: cannot reset ignored signal");` return Continue(1). |
| Action body has syntax error | Detected at fire time via `process_line` — error printed by parser; trap dispatching continues. NOT detected at registration time (matches bash). |
| Invalid `-p` arg (e.g. `trap -p NOPE`) | `eprintln!("huck: trap: NOPE: invalid signal specification");` return Continue(1). |
| Too few args (e.g. `trap action`) | `eprintln!("huck: trap: usage: trap [-lp] [[arg] signal_spec ...]");` return Continue(1). |

## Testing

### Module unit tests (`src/traps.rs`, ~12 tests)

- `parse_trap_signal_name` — `"INT"` → `Real(2)`.
- `parse_trap_signal_sig_prefix` — `"SIGINT"` → `Real(2)`.
- `parse_trap_signal_number` — `"2"` → `Real(2)`.
- `parse_trap_signal_exit` — `"EXIT"` → `Exit`.
- `parse_trap_signal_unknown_errors`.
- `parse_trap_signal_kill_uncatchable_errors`.
- `parse_trap_signal_stop_uncatchable_errors`.
- `drain_pending_returns_signals_in_order` — set bits, drain, assert order.
- `dispatch_pending_traps_runs_registered_action` — register, simulate fire by setting bit, drain, verify action ran.
- `fire_exit_trap_removes_then_runs_action` — register, fire, verify trap is now absent.
- `fire_exit_trap_no_action_is_noop`.
- `clear_for_subshell_resets_traps_and_bitmask`.

### Builtin unit tests (`src/builtins.rs`, ~10 tests)

- `trap_no_args_prints_active_traps`.
- `trap_p_prints_one_signal_only_when_given_name`.
- `trap_l_prints_signal_table`.
- `trap_dash_resets_signal`.
- `trap_empty_action_ignores_signal`.
- `trap_action_signal_registers`.
- `trap_unknown_signal_errors_status_1`.
- `trap_kill_signal_errors_uncatchable`.
- `trap_exit_pseudo_signal_registers`.
- `is_special_builtin_trap_returns_true`.

### Integration tests (`tests/trap_integration.rs`, ~12 tests)

- `exit_trap_fires_on_normal_exit` — `trap 'echo bye' EXIT; exit 0` → `bye` in stdout.
- `exit_trap_sees_last_status` — `trap 'echo $?' EXIT; false; exit` → `1`.
- `exit_trap_fires_on_eof` — script ends without explicit `exit`.
- `exit_trap_fires_only_once` — recursive exit from action doesn't re-fire.
- `exit_trap_cleared_in_subshell` — `(echo child)` doesn't fire parent's EXIT.
- `sigint_trap_fires_action` — spawn huck, send SIGINT mid-script, verify action ran.
- `trap_dash_resets_handler` — set then reset, send signal, no action.
- `trap_empty_action_ignores_signal` — set ignore, send signal, no output.
- `trap_p_output_format` — exact bash format match for one registered trap.
- `trap_l_lists_signals` — output contains `2) INT` and `15) TERM`.
- `trap_uncatchable_kill_errors` — `trap 'foo' KILL` → stderr error, exit 1.
- `trap_in_function_persists_after_return` — trap set inside function still active after return (traps are shell-global, not function-scoped).

**Test isolation**: trap installs OS-level handlers. Each integration
test spawns a fresh `huck` process via `Command::new(huck_binary())`
(same pattern as v33/v34 integration tests), so handler state never
leaks between tests.

**Total new tests**: ~34. Baseline goes from 1311 → ~1345.

## Documentation

- `docs/bash-divergences.md`:
  - **M-22** status flips to `[partial v35]` (not `[fixed]`) with
    notes:
    - EXIT pseudo-signal supported; ERR/DEBUG/RETURN still
      `[deferred]` — tracked under M-22's same entry as
      "Out of scope (still open): ERR/DEBUG/RETURN pseudo-signals."
    - Real-signal traps cover huck's 15-name table; M-41 (limited
      signal set) still applies — same gap as `kill`.
    - Subshell trap-clear matches POSIX/bash.
  - Changelog row.
- `README.md`: new v35 row in the status table.

## Scope (in)

- `trap ACTION SIGNAL...`, `trap "" SIGNAL...`, `trap - SIGNAL...`,
  `trap`, `trap -p [SIGNAL...]`, `trap -l`.
- EXIT pseudo-signal.
- Real signals from huck's existing 15-name table, minus KILL/STOP.
- Three name forms: `INT`, `SIGINT`, `2`.
- Subshell trap-clear after fork.
- POSIX "ignored at entry → cannot trap" enforcement.
- Action body re-parsed each fire (late variable binding).

## Scope (out)

- **ERR/DEBUG/RETURN pseudo-signals** — deferred to a follow-up
  iteration. Track as a new doc entry or expand M-22's note.
- **Signal mask manipulation** — POSIX `trap` only changes
  disposition, not mask. Out of scope by design.
- **`trap action` with no signals** — bash treats as a syntax error;
  we match.
- **Trap inheritance into `exec`** — `exec` semantics in huck are
  minimal; out of scope.
- **POSIX-locale message formatting** — error strings are
  English-only.
- **Per-function trap scopes** — bash treats traps as shell-global,
  not function-local; we match.

## Implementation tasks (handoff to writing-plans)

| # | Task | Notes |
|---|---|---|
| 1 | `src/traps.rs` module skeleton + `TrapSignal` enum + name table + `parse_trap_signal` + unit tests | Pure data; no Shell wiring. |
| 2 | `Shell::traps` + `Shell::trap_pending` fields + `Shell::new()` init + `TRAP_PENDING` `OnceLock` + `drain_pending` + `dispatch_pending_traps` + `fire_exit_trap` + `clear_for_subshell` + subshell-fork integration | Storage + delivery scaffold. |
| 3 | Signal-handler installation: `install()`, `reset()`, SIGINT/SIGCHLD layering, ignored-at-startup enforcement | OS plumbing. |
| 4 | `builtin_trap` + `BUILTIN_NAMES` + `is_special_builtin` + dispatch arm + builtin unit tests + output formatting | User-facing surface. |
| 5 | REPL/executor integration: polling at the three checkpoints + EXIT firing at all five exit paths | Wiring. |
| 6 | Integration tests + docs (M-22 → fixed; M-41 reminder; changelog; README) + full-suite verify | Coverage + close-out. |

Process: subagent-driven per `[[huck-iteration-workflow]]` on
`v35-trap-builtin` branch. Final code-reviewer over the whole branch
diff before `merge --no-ff` into `main`.
