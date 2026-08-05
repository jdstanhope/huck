# v354 — unify the pending-unwind signals — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give huck's five "stop what you are doing" signals one named home and one reporter, without changing a single behaviour ([#466](https://github.com/jdstanhope/huck/issues/466)).

**Architecture:** The three shell-raised signals (`pending_discard`, `pending_fatal_status`, `pending_exit`) move into one `Unwind` struct on `Shell`, reachable only through methods. The two externally-raised ones (`sigint_flag`, `timeout_flag`) keep their `Arc<AtomicBool>` storage because a signal handler and a timer thread write them. One `pending_unwind(shell, phase)` becomes the single place that decides what stops a command and in what order — taking a phase, because the two existing checkpoints genuinely ask different questions and that asymmetry is preserved, not normalised.

**Tech Stack:** Rust 2024, crates `huck-engine` (interpreter) and `huck-cli` (REPL). Tests are `#[cfg(test)] mod tests` blocks, `tests/*.rs` integration binaries, and `tests/scripts/*_diff_check.sh` bash-differential harnesses.

**Spec:** `docs/superpowers/specs/2026-08-05-unwind-unification-design.md` — read it first, especially "The precedence that exists today".

## Global Constraints

- **This is a behaviour-preserving refactor. The gate is zero expected-value edits.** If a task requires changing what a test *expects*, the migration changed behaviour: revert and redo. **Mechanically updating how a test *reaches* state is not an expected-value edit** — `shell.pending_discard = true` becoming `shell.raise_discard()` inside a test is required and fine. The distinction: assertions must not change; access may.
- **A live bug found on the way is FILED, not fixed.** Fixing it here forfeits the gate. Open an issue with the two-shell repro and move on. (v353 found two this way; this iteration deliberately does not.)
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- **Formatting:** `cargo fmt --all` before every commit; CI enforces `--check`.
- **This box has 1 core and 1.9 GB.** Never `cargo test --workspace` — it OOM-kills the session. Build with `cargo build -p huck --bin huck`.
- **Engine lib tests run with 4 threads:** `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4`.
- **Guard sweeps:** `( ulimit -v 1500000; timeout 3000 tests/scripts/run_diff_checks.sh )`. The sweep count on `main` is **266**.
- **Branch:** `v354-unwind-unification`, cut from `main` at or after `0d011358`. Never push to `main`.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/huck-engine/src/shell_state.rs` | `Shell` state | Add `pub struct Unwind` + `unwind: Unwind` field + accessors; remove the three old fields and their takers |
| `crates/huck-engine/src/executor.rs` | dispatch, unwind, fork | Add `pending_unwind` + `UnwindPhase`; `check_interrupt` becomes a wrapper; `finish_command` calls the reporter |
| `crates/huck-engine/src/expand.rs` | expansion | 8 discard + ~10 fatal sites → accessors |
| `crates/huck-engine/src/traps.rs`, `shell.rs`, `builtins.rs`, `param_expansion.rs`, `arith.rs`, `completion_spec.rs`, `exec_builder.rs`, `crates/huck-cli/src/repl.rs` | producers/consumers | field pokes → accessors |
| `crates/huck-engine/src/{expand,executor,shell_state}/tests.rs` | unit tests | mechanical access updates only |
| `docs/architecture.md` | module map | Document `Unwind` + the two-phase precedence table |
| `site/content/blog/<slug>.mdx` | blog | The "nothing changed, on purpose" entry |

---

### Task 1: `Unwind` exists and owns `exit`

Smallest signal first (19 sites, 7 files), so the shape is proven before the bulk moves.

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (field ~789, init ~1165, taker ~3267)
- Modify: `crates/huck-engine/src/{traps.rs,executor.rs,shell.rs,exec_builder.rs}`, `crates/huck-cli/src/repl.rs`

**Interfaces:**
- Consumes: nothing (first task)
- Produces: `pub struct Unwind` with `pub(crate) exit: Option<i32>`; `Shell::unwind: Unwind`; `Shell::raise_exit(&mut self, n: i32)`, `Shell::take_exit(&mut self) -> Option<i32>`, `Shell::exit_pending(&self) -> Option<i32>`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/huck-engine/src/shell_state.rs`:

```rust
#[test]
fn unwind_exit_slot_raises_peeks_and_takes() {
    let mut shell = Shell::new();
    assert_eq!(shell.exit_pending(), None, "clean shell has no request");
    shell.raise_exit(9);
    assert_eq!(shell.exit_pending(), Some(9), "peek does not consume");
    assert_eq!(shell.exit_pending(), Some(9), "still there after a peek");
    assert_eq!(shell.take_exit(), Some(9), "take consumes");
    assert_eq!(shell.exit_pending(), None, "gone after take");
}

#[test]
fn unwind_exit_slot_overwrites_rather_than_latching() {
    // #442: the LAST exit wins — an EXIT trap overrides an earlier request.
    let mut shell = Shell::new();
    shell.raise_exit(9);
    shell.raise_exit(7);
    assert_eq!(shell.take_exit(), Some(7));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 unwind_exit_slot`
Expected: FAIL — `no method named 'raise_exit' found`.

- [ ] **Step 3: Add the struct, the field and the accessors**

In `crates/huck-engine/src/shell_state.rs`, above `pub struct Shell`:

```rust
/// The shell's own "stop what you are doing" signals: set deep in expansion or
/// execution, consulted at a command boundary, converted into an
/// `ExecOutcome::Interrupted(..)`.
///
/// Slots are INDEPENDENT and may be set at once — a discard and a fatal can
/// both be raised during one command's expansion, and resolving between them
/// belongs to `executor::pending_unwind`, not to storage.
///
/// The externally-raised signals (`Shell::sigint_flag`, `Shell::timeout_flag`)
/// are deliberately NOT here: a signal handler and the timer thread write them
/// from outside this thread, so they must stay `Arc<AtomicBool>`.
#[derive(Debug, Default, Clone)]
pub struct Unwind {
    /// #442: an `exit N` performed BY a trap action. Overwritten, not latched
    /// — bash lets the last exit win.
    pub(crate) exit: Option<i32>,
}
```

Replace the `pending_exit` field on `Shell` with `pub unwind: Unwind,`, replace its initialiser with `unwind: Unwind::default(),`, and replace `take_pending_exit` with:

```rust
    /// Records an `exit N` performed by a trap action (#442). Overwrites: the
    /// last exit wins, which is how an EXIT trap overrides an earlier request.
    pub fn raise_exit(&mut self, n: i32) {
        self.unwind.exit = Some(n);
    }

    /// Peeks at a pending trap-action exit without consuming it.
    pub fn exit_pending(&self) -> Option<i32> {
        self.unwind.exit
    }

    /// Consumes a pending trap-action exit. Called at the run boundaries —
    /// the top-level reducer, the REPL and the forked-child exit path — AFTER
    /// the EXIT trap has had its chance to overwrite it.
    pub fn take_exit(&mut self) -> Option<i32> {
        self.unwind.exit.take()
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 unwind_exit_slot`
Expected: PASS (2 tests).

- [ ] **Step 5: Migrate the call sites**

`cargo build -p huck --bin huck` and fix each error. The expected edits, all mechanical:

| file | was | becomes |
|---|---|---|
| `traps.rs` (`run_trap_action`) | `shell.pending_exit = Some(n)` | `shell.raise_exit(n)` |
| `traps.rs` (`clear_for_subshell`) | `shell.pending_exit = None` | `shell.unwind = Default::default()` |
| `executor.rs` (`check_interrupt`) | `if let Some(n) = shell.pending_exit` | `if let Some(n) = shell.exit_pending()` |
| `executor.rs` (`finish_command`, ×2) | `if let Some(n) = shell.pending_exit` | `if let Some(n) = shell.exit_pending()` |
| `executor.rs` (`debug_trap_gate`) | `match shell.pending_exit` | `match shell.exit_pending()` |
| `executor.rs` (fork child) | `shell.take_pending_exit()` | `shell.take_exit()` |
| `shell.rs`, `repl.rs`, `exec_builder.rs` | `take_pending_exit()` | `take_exit()` |

- [ ] **Step 6: Verify nothing changed**

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 1200 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
cargo build -p huck --bin huck
for h in trap_action_exit subshell_exit_trap trap_action_status trap_reentrancy; do
  printf '%-24s ' $h
  ( ulimit -v 1500000; timeout 300 bash tests/scripts/${h}_diff_check.sh 2>&1 | tail -1 )
done
```

Expected: engine lib green (2 more tests than before), `trap_action_exit` 28/28, `subshell_exit_trap` 20/20, `trap_action_status` 20/20, `trap_reentrancy` 6/6 — **and no expected value edited.**

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(#466): Unwind owns the trap-action exit request

First of the three shell-raised signals to move into one named home. The
struct starts with the smallest slot so the shape is proven before the bulk
migrates; \`sigint_flag\` and \`timeout_flag\` stay Arc<AtomicBool> because a
signal handler and the timer thread write them.

No behavior change: same tests, same expected values.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `Unwind` owns `discard`

29 sites across 5 files, 4 of them in `expand/tests.rs` (mechanical access updates — allowed, see Global Constraints).

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs`, `expand.rs` (8 sites), `executor.rs` (12), `arith.rs` (1), `expand/tests.rs` (4)

**Interfaces:**
- Consumes: `Unwind` (Task 1)
- Produces: `Unwind::discard: bool`; `Shell::raise_discard(&mut self)`, `Shell::take_discard(&mut self) -> bool`

- [ ] **Step 1: Write the failing test**

Add to `#[cfg(test)] mod tests` in `crates/huck-engine/src/shell_state.rs`:

```rust
#[test]
fn unwind_discard_slot_is_independent_of_exit() {
    // The slots must NOT share storage: a discard and an exit can both be
    // pending, and `finish_command` resolves between them.
    let mut shell = Shell::new();
    shell.raise_discard();
    shell.raise_exit(9);
    assert!(shell.take_discard(), "discard survived raising an exit");
    assert_eq!(shell.exit_pending(), Some(9), "exit survived taking a discard");
    assert!(!shell.take_discard(), "take clears the discard");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 unwind_discard_slot`
Expected: FAIL — `no method named 'raise_discard' found`.

- [ ] **Step 3: Add the slot and accessors**

Add to `Unwind`:

```rust
    /// v312 (#3/#31): a fatal arithmetic-expansion or readonly-assignment
    /// error discards the CURRENT command (status 1) without exiting the
    /// shell.
    pub(crate) discard: bool,
```

and to `Shell`, replacing `take_pending_discard`:

```rust
    /// Marks the current command for discard (v312 #3/#31).
    pub fn raise_discard(&mut self) {
        self.unwind.discard = true;
    }

    /// Consumes the discard flag.
    pub fn take_discard(&mut self) -> bool {
        std::mem::take(&mut self.unwind.discard)
    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 unwind_discard_slot`
Expected: PASS.

- [ ] **Step 5: Migrate the call sites**

`cargo build -p huck --bin huck` and fix each error: `shell.pending_discard = true` → `shell.raise_discard()` (4 production sites: `expand.rs:1331`, `expand.rs:2003`, `executor.rs:4006`, `executor.rs:4016`), `shell.take_pending_discard()` → `shell.take_discard()`, and any `shell.pending_discard` read → `shell.unwind.discard` (still `pub(crate)` at this stage).

In `executor.rs` there is one combined read: `if shell.pending_discard || shell.pending_fatal_status.is_some()`. Leave the fatal half alone this task — it becomes `if shell.unwind.discard || shell.pending_fatal_status.is_some()`.

- [ ] **Step 6: Verify nothing changed**

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 1200 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
cargo build -p huck --bin huck
for h in arith_expansion_discard readonly_assign_discard arith_nonfatal pe_error_abort; do
  printf '%-26s ' $h
  ( ulimit -v 1500000; timeout 300 bash tests/scripts/${h}_diff_check.sh 2>&1 | tail -1 )
done
```

Expected: all green, **no expected value edited**. If `arith_expansion_discard` regresses, the `set_last_status(1)` ordering in `finish_command` was disturbed — revert.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(#466): Unwind owns the discard flag

Second of three. The new unit test pins the property that made a single
Option<Unwind> slot wrong: discard and exit are INDEPENDENT and can both be
pending, which is why storage keeps separate slots and the reporter resolves.

No behavior change: same tests, same expected values.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `Unwind` owns `fatal`

The bulk: 65 references across 11 files, ~21 writes and ~14 reads. Mechanical — no judgement — which is exactly why it is its own task rather than part of Task 4.

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs`, `expand.rs`, `executor.rs`, `builtins.rs`, `param_expansion.rs`, `shell.rs`, `completion_spec.rs`, `crates/huck-cli/src/repl.rs`, and the three `*/tests.rs` (access updates only)

**Interfaces:**
- Consumes: `Unwind` (Tasks 1-2)
- Produces: `Unwind::fatal: Option<i32>`; `Shell::raise_fatal(&mut self, n: i32)`, `Shell::take_fatal(&mut self) -> Option<i32>`, `Shell::fatal_pending(&self) -> bool`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn unwind_fatal_slot_coexists_with_the_others() {
    let mut shell = Shell::new();
    shell.raise_fatal(2);
    shell.raise_discard();
    shell.raise_exit(9);
    assert!(shell.fatal_pending(), "fatal is pending");
    assert!(shell.take_discard(), "discard unaffected");
    assert_eq!(shell.take_fatal(), Some(2), "fatal carries its status");
    assert!(!shell.fatal_pending(), "gone after take");
    assert_eq!(shell.exit_pending(), Some(9), "exit unaffected");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 unwind_fatal_slot`
Expected: FAIL — `no method named 'raise_fatal' found`.

- [ ] **Step 3: Add the slot and accessors**

Add to `Unwind`:

```rust
    /// A fatal expansion error's exit status: the shell exits with it once the
    /// current command unwinds (distinct from `discard`, which does not exit).
    pub(crate) fatal: Option<i32>,
```

and to `Shell`, replacing `take_pending_fatal_status`:

```rust
    /// Records a fatal expansion error's exit status.
    pub fn raise_fatal(&mut self, n: i32) {
        self.unwind.fatal = Some(n);
    }

    /// True when a fatal status is pending (does not consume).
    pub fn fatal_pending(&self) -> bool {
        self.unwind.fatal.is_some()
    }

    /// Consumes the pending fatal status.
    pub fn take_fatal(&mut self) -> Option<i32> {
        self.unwind.fatal.take()
    }
```

Keep `set_pending_fatal` (the POSIX-mode helper at `shell_state.rs:3275`) but have its body call `self.raise_fatal(status)` — its callers and its mode gating are unchanged.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 unwind_fatal_slot`
Expected: PASS.

- [ ] **Step 5: Migrate the call sites**

`cargo build -p huck --bin huck` repeatedly and fix each error:

- `shell.pending_fatal_status = Some(n)` → `shell.raise_fatal(n)` (~21 sites)
- `shell.pending_fatal_status.is_some()` → `shell.fatal_pending()` (~14 sites)
- `shell.take_pending_fatal_status()` → `shell.take_fatal()`
- `if let Some(status) = shell.pending_fatal_status` → `if let Some(status) = shell.unwind.fatal` (still `pub(crate)` this task)

- [ ] **Step 6: Verify nothing changed**

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 1200 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
cargo build -p huck --bin huck
( ulimit -v 1500000; timeout 1800 cargo build --release --locked --bin huck )
( ulimit -v 1500000; timeout 3000 tests/scripts/run_diff_checks.sh 2>&1 | tail -2 )
```

Expected: engine lib green, sweep **266 passed, 0 failed**, **no expected value edited**.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(#466): Unwind owns the fatal status

Third and largest of the three — ~21 writes and ~14 reads across 11 files,
all mechanical and compiler-enumerated. set_pending_fatal keeps its name and
its POSIX-mode gating; only its body moves.

No behavior change: sweep 266/266, zero expected-value edits.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: One reporter, and seal the slots

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`check_interrupt`, `finish_command`)
- Modify: `crates/huck-engine/src/shell_state.rs` (slots `pub(crate)` → private)

**Interfaces:**
- Consumes: all three slots and their accessors (Tasks 1-3)
- Produces: `pub(crate) enum UnwindPhase { Around, After }`; `pub(crate) fn pending_unwind(shell: &Shell, phase: UnwindPhase) -> Option<ExecOutcome>`

- [ ] **Step 1: Write the failing test**

Add to `crates/huck-engine/src/executor/tests.rs`:

```rust
#[test]
fn pending_unwind_around_ignores_shell_raised_signals() {
    // The Around phase is what `check_interrupt` asks: the atomics plus a
    // trap's exit. It must NOT report a discard or a fatal — those belong to
    // the After phase, and reporting them here would fire at sites that have
    // never consulted them.
    let mut shell = Shell::new();
    shell.raise_discard();
    shell.raise_fatal(2);
    assert!(
        pending_unwind(&shell, UnwindPhase::Around).is_none(),
        "Around must ignore discard and fatal"
    );
    shell.raise_exit(9);
    assert!(
        matches!(
            pending_unwind(&shell, UnwindPhase::Around),
            Some(ExecOutcome::Interrupted(InterruptReason::ExitRequested(9)))
        ),
        "Around reports a trap's exit"
    );
}

#[test]
fn pending_unwind_after_prefers_discard_then_fatal_then_exit() {
    // The documented precedence of the After phase, in one place.
    let mut shell = Shell::new();
    shell.raise_discard();
    shell.raise_fatal(2);
    shell.raise_exit(9);
    assert!(
        matches!(
            pending_unwind(&shell, UnwindPhase::After),
            Some(ExecOutcome::Interrupted(InterruptReason::DiscardCommand))
        ),
        "discard outranks fatal and exit"
    );
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 pending_unwind`
Expected: FAIL — `cannot find function 'pending_unwind'`.

- [ ] **Step 3: Write the reporter**

In `crates/huck-engine/src/executor.rs`, replacing the body of `check_interrupt`:

```rust
/// Which question a checkpoint is asking. The two differ TODAY and that
/// asymmetry is preserved deliberately (v354): normalising it would be a
/// behaviour change wearing a refactor's clothes.
pub(crate) enum UnwindPhase {
    /// Around a command — the six `check_interrupt` sites and the wait loops.
    /// SIGINT -> timeout -> exit. Never consults discard or fatal.
    Around,
    /// After a command produced `Continue(c)` — `finish_command` only.
    /// discard -> fatal -> exit. Never consults the atomics.
    After,
}

/// The single place that decides what stops a command, and in what order.
pub(crate) fn pending_unwind(shell: &Shell, phase: UnwindPhase) -> Option<ExecOutcome> {
    use std::sync::atomic::Ordering;
    match phase {
        UnwindPhase::Around => {
            // SIGINT is CONSUMED here (compare_exchange). When SIGINT is
            // trapped we return None having cleared it, so the trap action
            // runs instead of the command being interrupted.
            if shell
                .sigint_flag
                .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                if shell.trap_sigids.contains_key(&libc::SIGINT) {
                    return None;
                }
                return Some(ExecOutcome::Interrupted(InterruptReason::Sigint));
            }
            // The timeout flag stays SET — the builder's epilogue does a
            // single `swap(false)` at the run boundary to override to 124.
            if shell.timeout_flag.load(Ordering::Relaxed) {
                return Some(ExecOutcome::Interrupted(InterruptReason::Timeout));
            }
            // #442: peeked, not taken — consumed at the run boundary.
            // Reported last: SIGINT and a timeout are externally imposed and
            // outrank the script's own request.
            shell
                .exit_pending()
                .map(|n| ExecOutcome::Interrupted(InterruptReason::ExitRequested(n)))
        }
        UnwindPhase::After => {
            // v312 (#3/#49): the discard flavour wins if both it and a fatal
            // were raised by the same command. NOTE: peek only — the caller
            // consumes, because taking here would strand the `$?` = 1 write
            // that must accompany it (#351).
            if shell.unwind.discard {
                return Some(ExecOutcome::Interrupted(InterruptReason::DiscardCommand));
            }
            // A pending fatal returns Continue(c) — NOT an Interrupted — the
            // status is consumed later by the top-level reducer. The caller
            // supplies `c`, so this arm reports None and `finish_command`
            // keeps its own `fatal_pending()` check.
            if shell.fatal_pending() {
                return None;
            }
            shell
                .exit_pending()
                .map(|n| ExecOutcome::Interrupted(InterruptReason::ExitRequested(n)))
        }
    }
}

/// Around-phase checkpoint. Kept as a named wrapper so its six call sites read
/// unchanged.
pub(crate) fn check_interrupt(shell: &Shell) -> Option<ExecOutcome> {
    pending_unwind(shell, UnwindPhase::Around)
}
```

- [ ] **Step 4: Rewire `finish_command`**

`finish_command` keeps its own consumption and `$?` writes — the reporter only decides *what* is pending, never consumes. Replace its three inline checks so the ordering comes from the reporter:

```rust
    // v354: the reporter DECIDES (precedence in one place), the caller
    // CONSUMES (because the discard's take must pair with the `$?` = 1 write,
    // #351, which does not belong in a `&Shell` reporter).
    match pending_unwind(shell, UnwindPhase::After) {
        Some(ExecOutcome::Interrupted(InterruptReason::DiscardCommand)) => {
            shell.take_discard();
            shell.set_last_status(1);
            return Some(ExecOutcome::Interrupted(InterruptReason::DiscardCommand));
        }
        Some(other) => return Some(other),
        None => {}
    }
    if shell.fatal_pending() {
        return Some(ExecOutcome::Continue(c));
    }
    crate::traps::dispatch_pending_traps(shell);
    if let Some(o) = pending_unwind(shell, UnwindPhase::After) {
        return Some(o);
    }
```

Note the first `match` is where all three signals can be pending at once, so it is
the call that makes the `After` precedence load-bearing rather than decorative.
The `fatal_pending()` check stays a separate statement below it because the
fatal arm needs `c`, which the reporter does not have — that is why
`pending_unwind`'s `After` arm returns `None` for a pending fatal rather than
inventing an outcome.

and the post-ERR check likewise becomes `if let Some(o) = pending_unwind(shell, UnwindPhase::After) { return Some(o); }`.

- [ ] **Step 5: Seal the slots**

Change `Unwind`'s three fields from `pub(crate)` to private. `cargo build -p huck --bin huck` and route every resulting error through an accessor — those errors are the checklist of anything Tasks 1-3 missed. Add `pub(crate) fn discard_pending(&self) -> bool { self.unwind.discard }` if a read site needs it.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 pending_unwind`
Expected: PASS (2 tests).

- [ ] **Step 7: Verify nothing changed**

```bash
cargo fmt --all
( ulimit -v 1500000; timeout 1200 cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4 )
cargo build -p huck --bin huck
( ulimit -v 1500000; timeout 1800 cargo build --release --locked --bin huck )
( ulimit -v 1500000; timeout 3000 tests/scripts/run_diff_checks.sh 2>&1 | tail -2 )
for t in trap_integration trap_pseudo_signals_integration subshell_integration \
         wait_integration pipefail_integration sigint_abort_integration \
         pe_error_abort_integration arith_nonfatal_integration; do
  printf '%-40s ' $t
  ( ulimit -v 1500000; timeout 600 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 | grep "test result" )
done
```

Expected: sweep **266 passed, 0 failed**, all binaries green, **no expected value edited**.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(#466): one reporter for the pending-unwind signals

pending_unwind(shell, phase) is now the single place that decides what stops a
command and in what order. It takes a phase because the two checkpoints really
do ask different questions: Around is SIGINT -> timeout -> exit and never
consults discard/fatal; After is discard -> fatal -> exit and never consults
the atomics. That asymmetry is PRESERVED and now documented in one table
instead of implied by statement order in two functions.

check_interrupt stays as a named wrapper so its six call sites read unchanged.
The reporter never consumes: finish_command keeps its own takes and its \$?
writes, because the discard arm must pair with set_last_status(1) (#351).

Sealing Unwind's slots private is what proves the migration complete — the
compile errors were the checklist.

No behavior change: sweep 266/266, zero expected-value edits.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Verification, docs, blog

**Files:**
- Modify: `docs/architecture.md`
- Create: `site/content/blog/<slug>.mdx`

- [ ] **Step 1: bash-suite PASS-set diff**

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
( ulimit -v 2500000; timeout 3000 bash tests/bash-test-suite/runner.sh > /tmp/v354-suite.md 2>&1 )
grep -c '| PASS' /tmp/v354-suite.md      # expect 39

git stash
( ulimit -v 2500000; timeout 3000 bash tests/bash-test-suite/runner.sh > /tmp/main-suite.md 2>&1 )
git stash pop

diff <(grep '| PASS' /tmp/main-suite.md | sort) <(grep '| PASS' /tmp/v354-suite.md | sort)
```

Expected: **empty diff**, 39 both sides. ⚠️ The runner rebuilds the RELEASE binary, so it leaves `target/release/huck` built from whichever branch ran last — **rebuild before capturing any before/after output.** (This caused a stale-binary false reading in v353.)

- [ ] **Step 2: Update the architecture doc**

In `docs/architecture.md`, extend the `InterruptReason` paragraph with the storage split and the precedence table: three shell-raised signals in `Shell::unwind` (`Unwind { discard, fatal, exit }`, private slots, reached via `raise_*` / `take_*` / `*_pending`), two externally-raised ones staying `Arc<AtomicBool>` because a signal handler and the timer thread write them, and `pending_unwind(shell, phase)` as the single reporter — `Around` = SIGINT → timeout → exit, `After` = discard → fatal → exit.

- [ ] **Step 3: Capture before/after for the blog**

```bash
# The pre-v354 binary, built from main:
git stash && cargo build --release --locked --bin huck && cp target/release/huck /tmp/huck-pre-v354 && git stash pop
cargo build --release --locked --bin huck    # rebuild the branch binary

for f in 'set -e; trap "exit 9" ERR; false; echo after' 'v=$((1/0))
echo "rc=$?"' 'trap "exit 3" USR1; kill -USR1 $$; sleep 0.2; echo after'; do
  printf 'pre : '; ( ulimit -v 400000; timeout 5 /tmp/huck-pre-v354 -c "$f" 2>&1 | tr '\n' '|' ); echo
  printf 'post: '; ( ulimit -v 400000; timeout 5 ./target/release/huck -c "$f" 2>&1 | tr '\n' '|' ); echo
done
```

Expected: **identical output on both sides** — that identity IS the story of the post.

- [ ] **Step 4: Write the blog entry**

`site/content/blog/<slug>.mdx`, frontmatter `title` (≤120), `date: 2026-08-05`, `summary` (≤300), `tags`, `version: "v354"`, `draft: false`. The "nothing changed, on purpose" shape used by the front-end-rewrite post: show the identical before/after, then explain that five ways to say "stop" — with precedence implied by statement order across two functions — had already cost two real bugs (#454, #455), and that the fix is a named family plus one precedence table rather than any new behaviour. Validate:

```bash
cd site && export NVM_DIR="$HOME/.nvm" && . "$NVM_DIR/nvm.sh" && nvm use node >/dev/null \
  && ( ulimit -v 12000000; node_modules/.bin/velite --strict )
```

Expected: exit 0.

- [ ] **Step 5: Open the PR and hand over**

```bash
git push -u origin v354-unwind-unification
gh pr create --base main --title 'v354: unify the pending-unwind signals (#466)' --body '...'
```

The body must state: the storage split and why the atomics stayed, the preserved `Around`/`After` asymmetry, sweep 266/266, the bash-suite PASS-set diff result, **that zero expected values were edited**, and any issues filed rather than fixed. Then poll `gh pr checks <N>` until both workflows finish and pass. Do **not** self-merge — a `vNN` iteration is the user's to merge.

---

## Self-Review

**Spec coverage.** Constraints → Global Constraints (atomics stay: Tasks 1/4 code + doc; co-occurrence: Task 2's independence test; strict preservation: the gate in every task's verify step; file-don't-fix: Global Constraints). Design §1 `Unwind` + accessor table → Tasks 1-3, sealed in Task 4 Step 5. Design §2 reporter + phase table → Task 4 Steps 3-4, pinned by two unit tests. Design §3 (`finish_command` calls the reporter twice) → Task 4 Step 4's post-ERR check. Migration table → Tasks 1-4 in the same order. Verification items 1-5 → each task's verify step, plus Task 5 Steps 1-5. Non-goals → Global Constraints + the Task 4 commit message.

**Placeholders.** The only `...` is the `gh pr create --body` in Task 5 Step 5, whose required content is enumerated immediately below it.

**Type consistency.** `Unwind { discard, fatal, exit }` is introduced slot-by-slot in Tasks 1-3 and read as `shell.unwind.discard` / `shell.unwind.fatal` in Tasks 2-3 while still `pub(crate)`, then sealed in Task 4. Accessors are named identically everywhere they appear: `raise_exit` / `take_exit` / `exit_pending`, `raise_discard` / `take_discard`, `raise_fatal` / `take_fatal` / `fatal_pending`, plus the optional `discard_pending` added in Task 4 Step 5. `pending_unwind(shell, UnwindPhase::{Around,After}) -> Option<ExecOutcome>` is defined in Task 4 Step 3 and called with those exact names in Step 4 and in both unit tests.

**The division of labour, stated once:** the reporter DECIDES and never consumes; the caller CONSUMES. That split exists because the discard's `take` has to pair with the `$?` = 1 write (#351), which cannot live in a function holding `&Shell`. The alternative — a consuming reporter taking `&mut Shell` — would pull `$?` writes into it, a materially bigger behavioural risk for an iteration whose whole claim is that nothing changed.

**A reviewer's likely objection, pre-answered:** the `After` arm returns `None` for a pending fatal, which reads like an omission. It is not — the fatal arm must return `Continue(c)`, and `c` belongs to the caller, so reporting it would mean inventing a status. `finish_command` therefore keeps an explicit `fatal_pending()` statement immediately after the reporter call, in the same relative position it occupies today.
