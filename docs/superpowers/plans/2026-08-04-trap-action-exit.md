# v353 — a trap action's `exit` reaches the exit path — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `exit N` inside any trap action terminate the shell with N ([#442](https://github.com/jdstanhope/huck/issues/442)), and make a forked child run the EXIT trap it installed for itself ([#449](https://github.com/jdstanhope/huck/issues/449)).

**Architecture:** Trap fire sites currently discard the action's `ExecOutcome`. Centralise the five fire helpers behind one `run_trap_action`, record an `Exit(n)` outcome in `Shell::pending_exit`, and surface it through the existing interrupt checkpoint as `InterruptReason::ExitRequested(n)` so huck's current unwind carries it out of functions, loops, subshells and command substitutions to the normal exit path — which is required, because bash still runs the EXIT trap when another trap's `exit` ends the shell.

**Tech Stack:** Rust 2024, crates `huck-engine` (interpreter) and `huck-cli` (REPL). Tests are `#[cfg(test)] mod tests` blocks, `tests/*.rs` integration binaries, and `tests/scripts/*_diff_check.sh` bash-differential harnesses.

**Spec:** `docs/superpowers/specs/2026-08-04-trap-action-exit-design.md` — read it first; the twelve-row contract table there is the acceptance criteria.

## Global Constraints

- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- **Formatting:** run `cargo fmt --all` before every commit; CI enforces `cargo fmt --all --check`.
- **This box has 1 core and 1.9 GB.** Never run `cargo test --workspace` — it OOM-kills the session. Use `cargo test -p <crate>` and build the binary with `cargo build -p huck --bin huck`.
- **Engine lib tests must run with 4 threads**: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4`. Process-global fd operations collide with libtest only at >1 thread, which is what CI has.
- **Guard every harness sweep**: `( ulimit -v 1500000; timeout 3000 tests/scripts/run_diff_checks.sh )`.
- **Guard every bash/huck probe**: `( ulimit -v 400000; timeout 5 bash -c "$frag" | head -c 200 )`. An unbounded fragment has OOM-killed this box before — `trap "echo x; return 3" RETURN` loops forever in real bash.
- **Branch:** all work on `v353-trap-action-exit`, cut from `main`. Never push to `main`.
- **Do not touch the wait loop** (`builtins.rs` `wait_all`) — see spec §6. Its lateness is [#453](https://github.com/jdstanhope/huck/issues/453), out of scope.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/huck-engine/src/traps.rs` | trap table + the five fire helpers | Add `TrapActionResult` + `run_trap_action`; rewrite the five helpers to use it; set `pending_exit` |
| `crates/huck-engine/src/shell_state.rs` | `Shell` state | Add `pending_exit: Option<i32>` field + init + reset in `clear_for_subshell`'s caller path |
| `crates/huck-engine/src/builtins.rs` | `InterruptReason` / `ExecOutcome` enums | Add `ExitRequested(i32)` variant |
| `crates/huck-engine/src/executor.rs` | command dispatch, unwind, fork | `check_interrupt` reports the new reason; extract `finish_command`; new status arms; fire EXIT in the forked child |
| `crates/huck-engine/src/shell.rs` | top-level reducer + exit path | Honour `pending_exit` after `fire_exit_trap` |
| `crates/huck-cli/src/repl.rs` | interactive exit path | Same, for the REPL |
| `crates/huck-engine/src/exec_builder.rs` | embedding API epilogue | Same, alongside the timeout override |
| `tests/scripts/trap_action_exit_diff_check.sh` | #442 harness | Create |
| `tests/scripts/subshell_exit_trap_diff_check.sh` | #449 harness | Create |
| `docs/bash-divergences.md` | intentional divergences | Add the RETURN-trap-recursion entry |

---

### Task 1: Centralise trap-action execution

No behaviour change. Collapses the duplicated push/run/pop/restore dance so the later tasks add their logic in one place.

**Files:**
- Modify: `crates/huck-engine/src/traps.rs` (`fire_exit_trap` ~83, `fire_debug_trap` ~131, `fire_return_trap` ~230, `fire_pseudo_trap` ~300, `dispatch_pending_traps` ~60)
- Test: `crates/huck-engine/src/traps.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing (first task)
- Produces: `pub(crate) struct TrapActionResult { pub outcome: ExecOutcome, pub status: i32 }` and `pub(crate) fn run_trap_action(shell: &mut Shell, sig: TrapSignal, action: &str) -> TrapActionResult`

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` block in `crates/huck-engine/src/traps.rs`:

```rust
#[test]
fn run_trap_action_is_transparent_to_status_and_reports_action_status() {
    let mut shell = Shell::new();
    shell.set_last_status(42);
    // The action's own `$?` is 7; the surrounding `$?` must be restored to 42.
    let r = run_trap_action(&mut shell, TrapSignal::Err, "(exit 7)");
    assert_eq!(r.status, 7, "action status must be observable to the caller");
    assert_eq!(shell.last_status(), 42, "surrounding $? must be restored");
}

#[test]
fn run_trap_action_reports_exit_outcome() {
    let mut shell = Shell::new();
    let r = run_trap_action(&mut shell, TrapSignal::Err, "exit 9");
    assert!(
        matches!(r.outcome, ExecOutcome::Exit(9)),
        "expected Exit(9), got {:?}",
        r.outcome
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 run_trap_action`
Expected: FAIL — `cannot find function 'run_trap_action' in this scope`.

- [ ] **Step 3: Implement `run_trap_action`**

Add above `fire_exit_trap` in `crates/huck-engine/src/traps.rs`:

```rust
/// What a trap action left behind. `status` is the action's OWN `$?`, sampled
/// before the surrounding value is restored — `fire_debug_trap` needs it to
/// compute its `DebugDecision` and it is unrecoverable afterwards.
pub(crate) struct TrapActionResult {
    pub outcome: ExecOutcome,
    pub status: i32,
}

/// Runs one trap action: freezes `$BASH_COMMAND` for its duration (#287, via
/// `firing_traps`), keeps it transparent to `$?` (#437), and records an `exit`
/// the action performed so the request survives the discarded outcome (#442).
pub(crate) fn run_trap_action(
    shell: &mut Shell,
    sig: TrapSignal,
    action: &str,
) -> TrapActionResult {
    let saved_status = shell.last_status();
    shell.firing_traps.push(sig);
    let outcome = crate::shell::process_line(action, shell, false);
    shell.firing_traps.pop();
    let status = shell.last_status();
    shell.set_last_status(saved_status);
    TrapActionResult { outcome, status }
}
```

Note `ExecOutcome` is already imported in this file via `crate::builtins::ExecOutcome`; if not, add the import.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 run_trap_action`
Expected: PASS (2 tests).

- [ ] **Step 5: Rewrite the five fire helpers to call it**

`fire_exit_trap` — keep the `remove` (self-removal before running is what makes `exit` inside the EXIT trap non-recursive):

```rust
pub fn fire_exit_trap(shell: &mut Shell) {
    let action = match shell.traps.remove(&TrapSignal::Exit) {
        Some(Some(text)) => text,
        _ => return,
    };
    let _ = run_trap_action(shell, TrapSignal::Exit, &action);
}
```

`fire_pseudo_trap` (ERR + RETURN) — the explicit save/restore added by #437 now lives in the helper:

```rust
fn fire_pseudo_trap(shell: &mut Shell, sig: TrapSignal) {
    if shell.firing_traps.contains(&sig) {
        return;
    }
    let action = match shell.traps.get(&sig) {
        Some(Some(text)) => text.clone(),
        _ => return,
    };
    let _ = run_trap_action(shell, sig, &action);
}
```

`fire_debug_trap` — **this is the risky one**. It must read the action's status from `TrapActionResult::status`, NOT from `shell.last_status()` (which is now the restored, pre-action value). Replace the body's action-running section so that:

```rust
    let r = run_trap_action(shell, TrapSignal::Debug, &action);
    shell.current_lineno = saved_lineno;
    shell.eval_frame = prev_frame;
    let decision = debug_decision(shell.extdebug(), r.status, in_subroutine);
    decision
```

Keep the existing `$LINENO` reframing (`prev_frame` / `saved_lineno`) exactly as it is; only the action-running and status-reading lines change. Delete the now-duplicated `saved_status` / `set_last_status(saved_status)` lines it used to carry.

`dispatch_pending_traps` — replace its inline push/run/pop with `let _ = run_trap_action(shell, TrapSignal::Real(sig), &action);`.

- [ ] **Step 6: Verify no behaviour changed**

Run, in order:

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 1200 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
for t in trap_integration trap_pseudo_signals_integration functions_integration; do
  ( ulimit -v 1500000; timeout 600 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 )
done
cargo build -p huck --bin huck
for h in trap_reentrancy debug_firing_points functrace trap_listing_format \
         return_trap_function err_trap_function trap_action_status; do
  ( ulimit -v 1500000; timeout 300 tests/scripts/${h}_diff_check.sh | tail -2 )
done
```

Expected: engine lib 1971 passed / 0 failed; all three integration binaries green; every harness `Fail: 0`. **Any expected-value edit means this refactor changed behaviour — revert and redo.**

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/traps.rs
git commit -m "refactor(#442): run every trap action through one helper

The five fire helpers each duplicated push firing_traps / process_line / pop,
and only two of them carried the \$? save-restore added in #437 — which is why
that leak existed in ERR and RETURN but not DEBUG. One \`run_trap_action\`
returns the action's outcome AND its own status, the latter because
\`fire_debug_trap\` computes its DebugDecision from it and the restore would
otherwise hide it.

No behaviour change: same tests, same expected values.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Extract `finish_command()`

No behaviour change. `run_andor_group` runs the same post-command epilogue twice; the later tasks add one check to it, and it must land once.

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`run_andor_group` ~308-429)

**Interfaces:**
- Consumes: nothing from Task 1
- Produces: `fn finish_command(cmd: &Command, c: i32, is_last: bool, err_armed: bool, shell: &mut Shell) -> Option<ExecOutcome>` — `Some(outcome)` means the caller returns it immediately; `None` means carry on.

- [ ] **Step 1: Read both copies before changing anything**

Run: `sed -n '308,429p' crates/huck-engine/src/executor.rs`

The two blocks differ in exactly two ways: `is_last` is `rest.is_empty()` in the first and `i + 1 == rest.len()` in the second, and the first has an extra `shell.set_last_status(1)` in its discard arm. Both differences are already parameters or belong inside the helper.

- [ ] **Step 2: Write the characterisation test**

This refactor's test is the existing suite, but add one test that pins the ordering the helper must preserve, in the `#[cfg(test)] mod tests` block of `crates/huck-engine/src/executor.rs`:

```rust
#[test]
fn err_trap_fires_before_errexit_exits() {
    // ERR must run before errexit ends the shell, and $? must survive the
    // action (#437). `set -e; trap 'echo E' ERR; false` prints E, exits 1.
    let mut shell = Shell::new();
    shell.shell_options.errexit = true;
    let _ = crate::shell::process_line("trap 'true' ERR", &mut shell, false);
    let out = crate::shell::process_line("false", &mut shell, false);
    assert!(
        matches!(out, ExecOutcome::Exit(1)),
        "expected Exit(1) from errexit, got {out:?}"
    );
}
```

- [ ] **Step 3: Run it to verify it passes now**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 err_trap_fires_before_errexit`
Expected: PASS — this is a characterisation test; it must pass before AND after the extraction.

- [ ] **Step 4: Extract the helper**

Add above `run_andor_group` in `crates/huck-engine/src/executor.rs`:

```rust
/// The post-command epilogue shared by every element of an and-or list: status
/// propagation, the two pending-unwind checks, signal-trap dispatch, the ERR
/// trap and errexit. Returns `Some(outcome)` when the caller must return it.
///
/// `err_armed` is the pre-command snapshot of whether the ERR trap was set
/// (#444: bash's `was_error_trap`) — a command that INSTALLS the ERR trap is
/// not caught by it. `is_last` is bash's and-or-list rule: only the
/// syntactically last command in the list fires ERR / errexit.
fn finish_command(
    cmd: &Command,
    c: i32,
    is_last: bool,
    err_armed: bool,
    shell: &mut Shell,
) -> Option<ExecOutcome> {
    shell.set_last_status(c);
    if shell.take_pending_discard() {
        shell.set_last_status(1);
        return Some(ExecOutcome::Interrupted(InterruptReason::DiscardCommand));
    }
    if shell.pending_fatal_status.is_some() {
        return Some(ExecOutcome::Continue(c));
    }
    crate::traps::dispatch_pending_traps(shell);
    if c != 0 && shell.err_suppressed_depth == 0 && is_last && !is_negated_pipeline(cmd) {
        if err_armed {
            crate::traps::fire_err_trap(shell);
        }
        if let Some(out) = maybe_errexit(shell, c) {
            return Some(out);
        }
    }
    None
}
```

- [ ] **Step 5: Call it from both arms**

In the `first` arm, replace everything from `shell.set_last_status(c);` through the errexit block with:

```rust
        if let Some(out) = finish_command(first, c, rest.is_empty(), err_armed_first, shell) {
            return out;
        }
```

In the `rest` loop, replace the same span with:

```rust
                if let Some(out) =
                    finish_command(command, c, i + 1 == rest.len(), err_armed, shell)
                {
                    return out;
                }
```

Leave `check_interrupt` and the control-flow-outcome match where they are — they run before `$?` is set and are not part of the epilogue.

- [ ] **Step 6: Prove it changed nothing**

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 1200 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
cargo build -p huck --bin huck
( ulimit -v 1500000; timeout 1800 cargo build --release --locked --bin huck )
( ulimit -v 1500000; timeout 3000 tests/scripts/run_diff_checks.sh | tail -2 )
```

Expected: engine lib green, sweep **263 passed, 0 failed**, and **zero edits to any expected value**. If a test needed changing, the extraction changed behaviour — revert and redo.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "refactor(#442): extract finish_command from run_andor_group

The post-command epilogue was written twice — 21 and 24 lines differing only
in how is_last is computed and one stray set_last_status(1). The pending-exit
check lands there next, and it must land once.

No behaviour change: sweep 263/263, zero expected-value edits.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `pending_exit` + `ExitRequested` — #442

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (field near `pending_discard` ~779, init ~1168)
- Modify: `crates/huck-engine/src/builtins.rs` (`InterruptReason` ~12)
- Modify: `crates/huck-engine/src/traps.rs` (`run_trap_action` from Task 1)
- Modify: `crates/huck-engine/src/executor.rs` (`check_interrupt` ~130, `finish_command` from Task 2, outcome→status match ~7966)
- Modify: `crates/huck-engine/src/shell.rs` (~340-355), `crates/huck-cli/src/repl.rs` (~373), `crates/huck-engine/src/exec_builder.rs` (epilogue)

**Interfaces:**
- Consumes: `run_trap_action` (Task 1), `finish_command` (Task 2)
- Produces: `Shell::pending_exit: Option<i32>`, `Shell::take_pending_exit(&mut self) -> Option<i32>`, `InterruptReason::ExitRequested(i32)`

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/trap_action_exit_diff_check.sh` — copy the `check()` scaffold from `tests/scripts/trap_action_status_diff_check.sh`, then:

```bash
# --- all five trap kinds honour `exit N` --------------------------------
check "EXIT kind"        'trap "exit 9" EXIT; true'
check "ERR kind"         'trap "exit 9" ERR; false; echo after'
check "DEBUG kind"       'trap "exit 9" DEBUG; echo a; echo after'
check "RETURN kind"      'set -T; f() { trap "exit 9" RETURN; :; }; f; echo after'
check "signal kind"      'trap "exit 9" USR1; kill -USR1 $$; sleep 0.2; echo after'

# --- bare exit uses $? at that moment -----------------------------------
check "bare exit EXIT"   'trap "exit" EXIT; (exit 4); true'
check "bare exit ERR"    'trap "exit" ERR; (exit 6); echo after'

# --- the unwind escapes every nesting form ------------------------------
check "from a function"  'trap "exit 9" ERR; f() { false; }; f; echo after'
check "from a loop"      'trap "exit 9" ERR; while true; do false; done; echo after'
check "from a subshell"  'trap "exit 9" ERR; ( false ); echo after'
check "from a comsub"    'trap "exit 9" ERR; x=$(false); echo after'
check "during wait"      'trap "exit 9" USR1; ( sleep 0.1; kill -USR1 $$ ) & wait; echo after'

# --- ordering rules -----------------------------------------------------
check "EXIT still fires" 'trap "echo E" EXIT; trap "exit 9" ERR; false'
check "last exit wins"   'trap "exit 7" EXIT; trap "exit 9" ERR; false'
check "exit in EXIT"     'trap "echo E; exit 7" EXIT; true'
check "beats errexit"    'set -e; trap "exit 9" ERR; false; echo after'
check "errexit alone"    'set -e; trap "echo E" ERR; false; echo after'
check "DEBUG pre-empts"  'trap "echo D; exit 3" DEBUG; trap "exit 9" ERR; false'

# --- traps that do NOT exit are unchanged -------------------------------
check "plain ERR"        'trap "echo E" ERR; false; echo after'
check "plain EXIT"       'trap "echo E" EXIT; true'
check "status survives"  'trap "echo E" ERR; false; echo "rc=$?"'
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo build -p huck --bin huck && ( ulimit -v 1500000; timeout 300 tests/scripts/trap_action_exit_diff_check.sh | tail -4 )`
Expected: FAIL on the five trap-kind rows and the ordering rows (huck prints `after`, rc 0).

- [ ] **Step 3: Add the state and the enum variant**

`crates/huck-engine/src/shell_state.rs`, next to `pending_discard`:

```rust
    /// #442: an `exit N` performed by a trap action. Set by
    /// `traps::run_trap_action`, surfaced by `executor::check_interrupt` as
    /// `InterruptReason::ExitRequested`, and consumed at a run boundary — the
    /// same lifecycle as `timeout_flag`. Overwritten, not latched: bash lets
    /// the LAST exit win, which is how the EXIT trap overrides an earlier
    /// request from ERR (`trap "exit 7" EXIT; trap "exit 9" ERR; false` = 7).
    pub pending_exit: Option<i32>,
```

Initialise it to `None` in `Shell::new` alongside `pending_discard: false`, and add a taker next to `take_pending_discard`:

```rust
    pub fn take_pending_exit(&mut self) -> Option<i32> {
        self.pending_exit.take()
    }
```

In `crates/huck-engine/src/builtins.rs`, extend `InterruptReason`:

```rust
    /// #442: a trap action ran `exit N`. Unwinds like `DiscardCommand` but the
    /// shell DOES exit, with `n`, after the EXIT trap has run.
    ExitRequested(i32),
```

Add `crate::traps::clear_for_subshell`'s reset: `shell.pending_exit = None;` alongside `shell.err_suppressed_depth = 0;`.

- [ ] **Step 4: Record the request and report it**

In `run_trap_action` (Task 1), after `let outcome = ...`:

```rust
    if let ExecOutcome::Exit(n) = outcome {
        // Overwrite, don't latch — the last exit wins (spec contract row 6).
        shell.pending_exit = Some(n);
    }
```

In `check_interrupt` (`executor.rs`), after the timeout check and before `None`:

```rust
    // #442: a trap action asked to exit. Reported last: SIGINT and a timeout
    // are externally imposed and outrank the script's own request.
    if let Some(n) = shell.pending_exit {
        return Some(ExecOutcome::Interrupted(InterruptReason::ExitRequested(n)));
    }
```

`check_interrupt` takes `&Shell`, so this peeks; the value is cleared where it is consumed (Step 5).

- [ ] **Step 5: Consume it at the three exit paths**

In `finish_command`, immediately after `crate::traps::dispatch_pending_traps(shell);` — so a signal trap dispatched here is honoured before errexit:

```rust
    if let Some(n) = shell.pending_exit {
        return Some(ExecOutcome::Interrupted(InterruptReason::ExitRequested(n)));
    }
```

In `crates/huck-engine/src/shell.rs`, the top-level reducer: add the arm to the match that turns an outcome into `code`,

```rust
        ExecOutcome::Interrupted(InterruptReason::ExitRequested(n)) => n,
```

and — critically — take the override **after** `fire_exit_trap`, because the EXIT trap may raise its own:

```rust
    crate::traps::fire_exit_trap(&mut shell);
    let code = shell.take_pending_exit().unwrap_or(code);
    shell.hangup_jobs();
    code
```

Apply the same "fire, then take the override" order in `crates/huck-cli/src/repl.rs` (~373) and in `exec_builder.rs`'s epilogue next to the timeout override.

- [ ] **Step 6: Run the harness to verify it passes**

Run: `cargo build -p huck --bin huck && ( ulimit -v 1500000; timeout 300 tests/scripts/trap_action_exit_diff_check.sh | tail -4 )`
Expected: `Fail: 0` across all 21 fragments.

- [ ] **Step 7: Run the regression set**

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 1200 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
for t in trap_integration trap_pseudo_signals_integration wait_integration \
         subshell_integration pipefail_integration set_options_integration; do
  printf '%-38s ' $t
  ( ulimit -v 1500000; timeout 600 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 | grep "test result" )
done
```

Expected: all green. A test asserting that a trap action does NOT exit is asserting the old divergent behaviour — check it against real bash before "fixing" the code.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "fix(#442): a trap action's exit terminates the shell

All five trap kinds discarded the action's ExecOutcome, so \`trap 'exit 1' ERR\`
— the standard abort-on-error idiom — silently continued. run_trap_action now
records the request in Shell::pending_exit, check_interrupt surfaces it as
InterruptReason::ExitRequested(n), and the existing unwind carries it out of
functions, loops, subshells and command substitutions.

The request is consumed AFTER fire_exit_trap at each exit path, so the EXIT
trap still runs when another trap ends the shell, and its own exit overrides
(bash: \`trap 'exit 7' EXIT; trap 'exit 9' ERR; false\` is 7).

tests/scripts/trap_action_exit_diff_check.sh: 21 byte-identical fragments.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: A forked child fires its own EXIT trap — #449

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`fork_and_run_in_subshell` child branch ~7961-7983)
- Create: `tests/scripts/subshell_exit_trap_diff_check.sh`

**Interfaces:**
- Consumes: `Shell::take_pending_exit` and `fire_exit_trap` (Task 3)
- Produces: nothing for later tasks

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/subshell_exit_trap_diff_check.sh` with the same `check()` scaffold, then:

```bash
# --- a child-installed EXIT trap fires, in every child kind -------------
check "plain subshell"   '( trap "echo t" EXIT; echo b )'
check "background"       '( trap "echo t" EXIT; echo b ) & wait'
check "pipeline stage"   '{ trap "echo t" EXIT; echo x; } | cat'
check "command sub"      'echo "[$( trap "echo t" EXIT; echo b )]"'
check "nested subshell"  '( ( trap "echo t" EXIT; echo b ) )'

# --- status propagation -------------------------------------------------
check "trap exits child" '( trap "exit 9" EXIT; echo b ); echo "rc=$?"'
check "explicit exit"    '( trap "echo t" EXIT; exit 3 ); echo "rc=$?"'
check "body status kept" '( trap "echo t" EXIT; false ); echo "rc=$?"'
check "comsub status"    'x=$( trap "echo t; exit 9" EXIT; echo b ); echo "[$x] rc=$?"'

# --- the parent trap is NOT inherited (must not regress) ----------------
check "not inherited"    'trap "echo T" EXIT; ( echo sub )'
check "not in comsub"    'trap "echo T" EXIT; echo "[$(echo body)]"'
check "parent unchanged" 'trap "echo T" EXIT; ( exit 5 ); echo "rc=$?"'
check "child then parent" 'trap "echo P" EXIT; ( trap "echo C" EXIT; echo b )'
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo build -p huck --bin huck && ( ulimit -v 1500000; timeout 300 tests/scripts/subshell_exit_trap_diff_check.sh | tail -4 )`
Expected: FAIL on every "child-installed" row (huck prints the body but never `t`).

- [ ] **Step 3: Fire the trap in the child**

In `fork_and_run_in_subshell`, between the outcome→status match and `flush_stdout()`:

```rust
        // #449: a child runs the EXIT trap it installed for ITSELF (the
        // inherited one was cleared by clear_for_subshell above). Fired before
        // flush_stdout so the action's output is captured by `$( )`, and its
        // own `exit N` overrides the body's status (#442).
        crate::traps::fire_exit_trap(shell);
        let status = shell.take_pending_exit().unwrap_or(status);
        let status = status.rem_euclid(256);
```

Delete the pre-existing `let status = status.rem_euclid(256);` line so the modulus is applied once, after the override.

- [ ] **Step 4: Run the harness to verify it passes**

Run: `cargo build -p huck --bin huck && ( ulimit -v 1500000; timeout 300 tests/scripts/subshell_exit_trap_diff_check.sh | tail -4 )`
Expected: `Fail: 0` across all 13 fragments.

- [ ] **Step 5: Run the regression set**

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 1200 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
for t in subshell_integration cmdsub_subshell_integration pipeline_subshell_integration \
         process_sub coproc_integration wait_integration disown_pid_integration; do
  printf '%-38s ' $t
  ( ulimit -v 1500000; timeout 600 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 | grep "test result" )
done
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "fix(#449): a forked child fires the EXIT trap it installed

All four child kinds — ( ), &, a pipeline stage and \$( ) — funnel through
fork_and_run_in_subshell, so one site covers them. Fired before flush_stdout
so a captured child's trap output lands in the capture, and its own exit N
overrides the body's status.

The inherited case is unchanged and pinned: a child still does NOT run the
parent's EXIT trap.

tests/scripts/subshell_exit_trap_diff_check.sh: 13 byte-identical fragments.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Full verification + docs

**Files:**
- Modify: `docs/bash-divergences.md`
- Modify: `docs/architecture.md` (trap section, if it describes the fire path)

- [ ] **Step 1: Build both binaries and run the whole sweep**

```bash
cargo build -p huck --bin huck
( ulimit -v 1500000; timeout 1800 cargo build --release --locked --bin huck )
( ulimit -v 1500000; timeout 3000 tests/scripts/run_diff_checks.sh | tail -3 )
```

Expected: **265 passed, 0 failed** (263 + the two new harnesses).

- [ ] **Step 2: bash test-suite PASS-set diff**

The runner is `tests/bash-test-suite/runner.sh` and it requires bash 5.2.21 **sources** (not just the binary), located by `$BASH_SOURCE_DIR`; with that unset it skips. See `tests/bash-test-suite/README.md`. Running all 82 categories writes a Markdown table.

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21     # per tests/bash-test-suite/README.md
bash tests/bash-test-suite/runner.sh > /tmp/v353-suite.md 2>&1
grep -c '| PASS' /tmp/v353-suite.md

git stash
bash tests/bash-test-suite/runner.sh > /tmp/main-suite.md 2>&1
git stash pop

diff <(grep '| PASS' /tmp/main-suite.md | sort) <(grep '| PASS' /tmp/v353-suite.md | sort)
```

Expected: **empty diff**, PASS count still **39**. `dbg-support`, `dbg-support2`, `errexit`, `trap` and `exit` are the categories at risk. A single category can be re-run alone with `HUCK_BASH_TEST_CATEGORY=trap bash tests/bash-test-suite/runner.sh`.

If `$BASH_SOURCE_DIR` cannot be provided on this machine, say so in the PR rather than silently skipping this gate — the user asked for the bash-suite comparison explicitly, and a skipped run must not be reported as a pass.

- [ ] **Step 3: Record the by-design divergence**

Add to `docs/bash-divergences.md`:

```markdown
### `return` inside a RETURN trap does not exhaust memory

`f() { trap "echo x; return 3" RETURN; return 7; }; f` re-enters the RETURN
trap in bash until it dies with `xmalloc: cannot allocate 16 bytes` (exit 2).
huck's recursion guard runs the action once and continues. Reproducing an
out-of-memory crash is not a compatibility goal.
```

Then open and immediately close a `by-design` issue recording it:

```bash
gh issue create --title 'By design: `return` inside a RETURN trap does not exhaust memory' \
  --label divergence --label by-design --body '...' 
gh issue close <N> --comment 'Kept by design; recorded in docs/bash-divergences.md.'
```

- [ ] **Step 4: Open the PR**

```bash
git push -u origin v353-trap-action-exit
gh pr create --base main --title 'v353: a trap action'"'"'s exit reaches the exit path (#442, #449)' --body '...Closes #442
Closes #449'
```

The body must state: the contract table rows covered, sweep 265/265, the bash-suite PASS-set diff result, and that #453 (a trapped signal not interrupting `wait`) and the v354 flag unification are deliberately out of scope.

- [ ] **Step 5: Wait for CI, then hand to the user**

Poll `gh pr checks <N>` until **both** workflows report a finished, passing run. Do **not** self-merge — this is a `vNN` iteration, so the user reviews and merges.

---

## Self-Review

**Spec coverage.** Contract rows 1-5 → Task 3 harness rows 1-7 and 13-15; row 6 (last writer wins) → Task 3 "last exit wins"; row 7 (beats errexit) → Task 3 "beats errexit"; rows 8-10 (child kinds, child status, capture content) → Task 4; row 11 (during wait) → Task 3 "during wait", with spec §6 explaining why no wait-loop change is needed; row 12 (inherited unchanged) → Task 4's last four rows. Design §1 → Task 1, §2-3 → Task 3, §4 → Task 2, §5 → Task 4, §6 → no task by design. Verification items 1-6 → Task 3 step 7, Task 4 step 5, Task 5 steps 1-2, and Task 2 step 6's zero-edit gate. Non-goals → Task 5 step 3.

**Placeholders.** The only `...` are in the two `gh` command bodies in Task 5, where the required content is spelled out in prose immediately below each.

**Type consistency.** `TrapActionResult { outcome, status }` is produced in Task 1 and consumed by name in Tasks 1 and 3. `finish_command(cmd, c, is_last, err_armed, shell) -> Option<ExecOutcome>` is produced in Task 2 and called with those exact arguments in Task 2 step 5 and extended in Task 3 step 5. `pending_exit` / `take_pending_exit` / `ExitRequested(i32)` are defined in Task 3 and used in Task 4 with matching names.
