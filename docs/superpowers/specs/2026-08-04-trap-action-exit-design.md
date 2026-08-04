# v353 — a trap action's `exit` reaches the exit path — design

**Issues:** [#442](https://github.com/jdstanhope/huck/issues/442) — *`exit` inside
an ERR/DEBUG/RETURN trap action does not exit the shell* — and
[#449](https://github.com/jdstanhope/huck/issues/449) — *An EXIT trap set inside
a subshell or `$( )` never fires*. They are one root and ship together: both are
places where a trap action's outcome never reaches the code that decides how the
shell (or a forked child) terminates.

## Problem

Every trap fire site ends the same way:

```rust
shell.firing_traps.push(sig);
let _ = crate::shell::process_line(&action, shell, false);   // outcome discarded
shell.firing_traps.pop();
```

`process_line` returns an `ExecOutcome`. When the action ran `exit 9`, that
outcome is `ExecOutcome::Exit(9)` — and `let _ =` throws it away. So:

```
$ huck -c 'trap "exit 9" EXIT; true';                 echo $?    # 0, bash 9
$ huck -c 'trap "exit 9" ERR; false; echo after'                 # prints "after", bash exits 9
$ huck -c 'trap "exit 9" DEBUG; echo a; echo after'              # runs both, bash exits 9
$ huck -c 'f() { trap "exit 9" RETURN; :; }; f; echo after'      # prints "after", bash exits 9
$ huck -c 'trap "exit 9" USR1; kill -USR1 $$; sleep .2; echo after'  # prints "after", bash exits 9
```

All five trap kinds, and `trap 'exit 1' ERR` is a common abort-on-error idiom, so
this is a silent wrong-answer bug in careful scripts.

The second half is the same discarded-outcome problem one process out. A forked
child never fires the EXIT trap it installed for itself:

```
$ huck -c '( trap "echo t" EXIT; echo b )'                 # b        — bash: b, t
$ huck -c '{ trap "echo t" EXIT; echo x; } | cat'          # x        — bash: x, t
$ huck -c 'echo "[$( trap "echo t" EXIT; echo b )]"'       # [b]      — bash: [b, t]
$ huck -c '( trap "echo t" EXIT; echo b ) & wait'          # b        — bash: b, t
```

Fixing #449 without #442 would produce a subshell that runs its EXIT trap and
then ignores an `exit` inside it, which is why they are one iteration.

## The contract

Measured against bash 5.2.21 on 2026-08-04. This table is the acceptance
criteria; every row gets a harness fragment.

| case | bash |
|---|---|
| `exit N` in an EXIT / ERR / DEBUG / RETURN / signal trap | shell exits with N |
| bare `exit` in a trap action | exits with `$?` as of that moment |
| `exit N` in a trap, nested in a function / loop / subshell / `$( )` | unwinds all of it, exits N |
| a non-EXIT trap's `exit` terminates the shell | the EXIT trap **still fires first** |
| `exit N` inside the EXIT trap itself | that N wins; the trap does not re-fire |
| `trap "exit 7" EXIT; trap "exit 9" ERR; false` | **7** — the last writer wins |
| `set -e; trap "exit 9" ERR; false` | **9** — a trap's exit beats the errexit status |
| child-installed EXIT trap in `( )` / `&` / a pipeline stage / `$( )` | fires at that child's exit |
| child EXIT trap running `exit 9` | sets the **child's** status to 9 |
| `x=$( trap "echo t; exit 9" EXIT; echo b )` | captures `b\nt`, `$?` = 9 |
| `trap "exit 9" USR1; ( sleep .1; kill -USR1 $$ ) & wait` | exits 9 — the unwind reaches the wait loop |
| parent's EXIT trap inherited by a child | does **not** fire in the child (huck already matches) |

Two consequences drive the design. Because the EXIT trap must still fire when
another trap's `exit` ends the shell, the unwind **cannot** short-circuit to
`_exit` — it has to arrive at the normal exit path. And because the last `exit`
wins, `pending_exit` is overwritten on every write rather than latched.

## Design

### 1. `run_trap_action()` — one place where a trap action runs

`crates/huck-engine/src/traps.rs` has five fire helpers that each repeat the same
dance: push `firing_traps`, run the action, pop, and (since #437, in
`fire_pseudo_trap` and `fire_debug_trap` only) save and restore `$?`. That
duplication is why the `$?` leak existed in ERR and RETURN but not DEBUG. Adding
a second obligation to every site invites the same drift, so the sites collapse
first:

```rust
/// What an action left behind. `status` is the action's OWN `$?`, sampled
/// before the surrounding value is restored — `fire_debug_trap` needs it to
/// compute its `DebugDecision`, and it is unrecoverable afterwards.
pub(crate) struct TrapActionResult {
    pub outcome: ExecOutcome,
    pub status: i32,
}

/// Runs one trap action. Freezes $BASH_COMMAND via `firing_traps`, keeps the
/// action transparent to `$?`, and records an `exit` the action performed so
/// the exit request survives (see `Shell::pending_exit`).
pub(crate) fn run_trap_action(shell: &mut Shell, sig: TrapSignal, action: &str)
    -> TrapActionResult;
```

`fire_exit_trap`, `fire_pseudo_trap` (ERR + RETURN), `fire_debug_trap` and
`dispatch_pending_traps` all call it.

The `status` field is load-bearing and easy to lose: `fire_debug_trap` currently
computes `debug_decision(extdebug, shell.last_status(), in_subroutine)` *after*
the action and *before* restoring the saved value. Once the restore moves inside
the helper, reading `shell.last_status()` at the call site would yield the
pre-action status and silently disable extdebug's `SkipCommand` /
`ReturnFromSub` behaviour — a regression the DEBUG harnesses would catch only if
they exercise extdebug, so it is called out here rather than discovered later.

`fire_exit_trap` keeps removing the action **before** the call, which is what
makes `exit` inside the EXIT trap non-recursive without a special case.

### 2. `Shell::pending_exit: Option<i32>`

Set by `run_trap_action` whenever the action's outcome is `ExecOutcome::Exit(n)`,
overwriting any previous value. Cleared where it is consumed: the top-level exit
path, the interactive REPL's exit path, and the forked-child exit path.

This mirrors `timeout_flag`, which already "stays set — the builder's epilogue
does a single `swap(false)` at the run boundary" (`executor.rs:142`). Following
that precedent keeps `check_interrupt(&Shell)` immutable, so none of its six call
sites change shape.

`clear_for_subshell` resets it, so a request recorded before a fork cannot leak
into the child's state.

### 3. `InterruptReason::ExitRequested(i32)`

`check_interrupt` reports a pending exit as
`ExecOutcome::Interrupted(InterruptReason::ExitRequested(n))`. huck's existing
unwind already carries an `Interrupted` outcome out of functions, loops,
subshells and command substitutions exactly as it does `Timeout`, so no new
propagation is written — only new arms where an `Interrupted` outcome is turned
into a status:

- the top-level reducer in `shell.rs` (~line 340) → exit code `n`, **after**
  `fire_exit_trap` has run;
- `fork_and_run_in_subshell`'s outcome→status match (`executor.rs:7966`) → `n`;
- `ExecBuilder`'s epilogue, alongside the existing timeout override.

Check order inside `check_interrupt` stays SIGINT, then timeout, then
`ExitRequested`: an interactive interrupt and a timeout are both externally
imposed and outrank a script's own request.

### 4. `finish_command()` — the duplicated epilogue

`run_andor_group` runs the same post-command sequence twice, once for `first`
(`executor.rs:317-370`) and once per `rest` element (`executor.rs:382-424`) — 21
and 24 lines that differ only in how `is_last` is computed and one stray
`set_last_status(1)`. The sequence is: check interrupt → propagate control-flow
outcomes → set `$?` → pending discard → pending fatal → dispatch signal traps →
ERR fire → errexit.

It becomes one function taking the command, its outcome and `is_last`. The
pending-exit check then lands in one place, ordered so that **a pending exit
outranks the errexit status** (contract row 7).

This extraction is behaviour-preserving and is its own task, with a zero
expected-value-edit gate (see Verification).

### 5. The child exit path — one site

All four child kinds funnel through `fork_and_run_in_subshell`
(`executor.rs:7961-7983`): `Command::Subshell`, background commands, pipeline
stages, and `$( )` via `capture_via_fork` (`executor.rs:7815`). So #449 is a
single edit, in the child branch after the body runs and **before**
`flush_stdout()` — so the trap's output is captured (contract row 10):

```
run body → outcome → fire_exit_trap → status = pending_exit ?? outcome-status
         → flush_stdout → _exit(status)
```

The other three `libc::_exit` sites in `executor.rs` (5629, 8037, 8103) are
diagnostic-only children — command-not-found and failed pipeline stages — that
run no shell body and are untouched. The two `_exit` calls outside `executor.rs`
(`wait_loop.rs:398`, `stream_loop.rs:221`) are inside `#[test]` functions, not
production paths. So `fork_and_run_in_subshell` is the only place a forked child
ever runs shell code, and therefore the only place an EXIT trap can be owed.

### 6. The wait loop

`trap "exit 9" USR1; ( sleep .1; kill -USR1 $$ ) & wait` exits 9 in bash, so the
`wait` polling loop must notice `pending_exit` after dispatching a signal trap
and return an `ExitRequested` outcome rather than continuing to wait. This is the
one place the request is raised outside the executor's command boundaries, and
it gets a dedicated harness fragment.

## Data flow, worked

**`trap "exit 9" ERR; false` at top level.** `run_andor_group` runs `false` →
`finish_command` fires the ERR trap → `run_trap_action` sees `Exit(9)` and sets
`pending_exit = Some(9)` → `finish_command`'s interrupt check returns
`Interrupted(ExitRequested(9))` → propagates to the driver → the driver runs
`fire_exit_trap` (which may overwrite `pending_exit`) → exit code 9.

**`( trap "exit 9" EXIT; echo b )`.** The child forks; `clear_for_subshell` wipes
the inherited table; the body installs its own EXIT trap and prints `b`; the
child's exit path fires it; `run_trap_action` records 9; the child `_exit`s 9.
The parent's `$?` is 9 and its own EXIT trap is untouched.

**`trap "exit 7" EXIT; trap "exit 9" ERR; false`.** ERR sets 9 → unwind → driver
runs the EXIT trap → its action sets `pending_exit = Some(7)`, overwriting →
exit code 7, matching bash.

## Non-goals

- **`return` inside a RETURN trap.** Real bash re-enters the trap until it dies
  with `xmalloc: cannot allocate 16 bytes` (rc 2). huck's recursion guard runs
  the action once and continues. Reproducing memory exhaustion is not a
  compatibility goal: this ships as a `by-design` divergence, added to
  `docs/bash-divergences.md` with its own opened-and-closed issue.
- **Unifying the pending-unwind flags.** `sigint_flag`, `timeout_flag`,
  `pending_discard`, `pending_fatal_status` and the new `pending_exit` share one
  lifecycle and deserve one type and one checkpoint. That is **v354**, deliberately
  after this: changing when the shell exits *and* restructuring how every unwind
  is represented in one diff would make a regression impossible to attribute.
- **Inherited-trap semantics.** A child correctly does not fire the parent's EXIT
  trap today; unchanged.
- **#439** (DEBUG entry-unset) and **#445** (ERR double-firing inside `{ }` /
  `for` / `case`) stay open and out of scope.

## Verification

1. **New harnesses.** `trap_action_exit_diff_check.sh` — all five trap kinds ×
   nesting contexts (top level, function, loop, subshell, `$( )`, during `wait`)
   × `exit N` / bare `exit` / errexit interaction / the last-writer rule.
   `subshell_exit_trap_diff_check.sh` — the four child kinds, status propagation,
   capture content, and the inherited-trap rows that must **not** change.
2. **The existing sweep.** `tests/scripts/run_diff_checks.sh` — all 263 green,
   both binaries built first.
3. **Tests.** `cargo test -p huck-engine --lib -- --test-threads 4` (the 4-core
   setting is required — see the fd-arc CI trap), plus the trap, functions,
   subshell, pipeline, wait and job-control integration binaries run
   single-threaded.
4. **bash suite as a PASS-set diff.** Run the category runner and diff the entire
   PASS set against the 39/82 baseline — not the count, which can hide one
   category flipping each way. `dbg-support`, `dbg-support2`, `errexit`, `trap`
   and `exit` are the categories at risk.
5. **A zero-edit gate on the extraction.** The `finish_command()` task must not
   change a single expected value; proven the v343 way, by diffing every
   category's diff-line count against `origin/main` and requiring them identical.
   If a test needs updating, the extraction changed behaviour and is redone.
6. **CI green before merge** — the 4-core runner exposes races this 1-core box
   cannot.

## Staging

1. `run_trap_action()` — collapse the five fire helpers, no behaviour change.
2. `finish_command()` — extract the duplicated epilogue, no behaviour change
   (zero-edit gate).
3. `pending_exit` + `ExitRequested` + the three status arms — #442 for the four
   non-signal trap kinds.
4. The wait-loop checkpoint — #442 for signal traps.
5. The child exit path — #449.
6. Harnesses, sweep, bash-suite PASS-set diff, docs (`bash-divergences.md` entry
   + `by-design` issue for the RETURN-trap recursion).
