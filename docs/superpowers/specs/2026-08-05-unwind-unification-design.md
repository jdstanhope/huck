# v354 — unify the pending-unwind signals — design

**Issue:** [#466](https://github.com/jdstanhope/huck/issues/466) — *Architecture:
unify the five pending-unwind signals behind one type and one checkpoint.*

**Depends on:** v353 ([#442](https://github.com/jdstanhope/huck/issues/442) /
[#449](https://github.com/jdstanhope/huck/issues/449), merged as `49c3f0a2`),
which added the fifth signal and created the `finish_command` seam this works
through.

## Problem

huck has five ways to say "stop what you are doing". Each has its own storage,
its own check, and a precedence relative to the others that is implied by the
order statements happen to appear in:

| signal | type | raised by | refs |
|---|---|---|---|
| `sigint_flag` | `Arc<AtomicBool>` | a signal handler (async-signal-safe) | ~11 |
| `timeout_flag` | `Arc<AtomicBool>` | the timer thread | ~12 |
| `pending_discard` | `bool` | the shell, mid-command (v312 arith/readonly discard) | 29 / 5 files |
| `pending_fatal_status` | `Option<i32>` | the shell, mid-command | 65 / 11 files |
| `pending_exit` | `Option<i32>` | a trap action (v353) | 19 / 7 files |

They share one lifecycle — set deep, checked at a command boundary, converted
into `ExecOutcome::Interrupted(..)` — but nothing in the code says so. A sixth
signal would be added the same way the fifth was: as another loose field that
some checkpoints happen to consult.

That shape has already cost two bugs, both found in v353 the moment duplicated
code was consolidated and neither reported by a user:
[#454](https://github.com/jdstanhope/huck/issues/454) (a `$?` save reached two
of five copies of "run a trap action") and
[#455](https://github.com/jdstanhope/huck/issues/455) (a status fix reached one
of two copies of the post-command epilogue).

## Constraints

**The two atomics cannot move.** `sigint_flag` is written from a signal
handler, where only async-signal-safe operations are legal — no allocation, no
locks. `timeout_flag` is written from the timer thread. Both must stay
`Arc<AtomicBool>` and stay shared. Any design that folds them into a
payload-carrying field is wrong, and this one does not attempt it.

**The three shell-raised signals can co-occur.** `pending_discard` and
`pending_fatal_status` are both written from `expand.rs` during one command's
expansion, and `finish_command`'s existing comment anticipates the collision
("the discard flavor wins if both were somehow raised by the same command").
Storage must therefore keep independent slots; collapsing them into a single
`Option<Unwind>` would silently turn "discard wins" into "last writer wins".

**Strict preservation.** This is a behaviour-preserving refactor in the v343
mould. The gate is zero expected-value edits (see Verification). If a task
uncovers a live bug the way v353's consolidations twice did, it is FILED and
NOT fixed inline — a behaviour fix would forfeit the gate that makes the
refactor provable.

## The precedence that exists today

Measured, not assumed. The two checkpoints ask different questions:

```
check_interrupt  (6 sites, around commands + wait loops)
    SIGINT → timeout → exit          never consults discard or fatal

finish_command   (per and-or element, after Continue(c))
    discard → fatal → signal traps → exit → ERR fire → exit → errexit
                                     never consults SIGINT or timeout
```

So precedence is **location-dependent**: with a discard and an exit both
pending, `finish_command` lets the discard win while the next `check_interrupt`
lets the exit win. This asymmetry is PRESERVED, not removed — normalising it
would be a behaviour change wearing a refactor's clothes. What changes is that
it becomes visible in one table instead of implied by statement order in two
functions.

## Design

### 1. `Unwind` — one named home for the three shell-raised signals

```rust
/// The shell's own "stop what you are doing" signals: set deep in expansion or
/// execution, consulted at a command boundary, converted into an
/// `ExecOutcome::Interrupted(..)`.
///
/// Slots are INDEPENDENT and may be set at once — a discard and a fatal can
/// both be raised during one command's expansion, and the resolution between
/// them belongs to the reporter, not to storage.
pub struct Unwind {
    discard: bool,
    fatal: Option<i32>,
    exit: Option<i32>,
}
```

`Shell` loses `pending_discard`, `pending_fatal_status` and `pending_exit`, and
gains `unwind: Unwind`. In the END STATE the slots are **private**, which is
what forces every one of the ~113 existing sites through a method. During the
migration they are `pub(crate)` so each signal can move in its own task; the
sealing is the last step of task 4, and the compile errors it produces are the
checklist for anything the earlier tasks missed.

| new method | replaces |
|---|---|
| `raise_discard()` | `shell.pending_discard = true` (4 sites) |
| `raise_fatal(n)` | `shell.pending_fatal_status = Some(n)` (21 sites) |
| `raise_exit(n)` | `shell.pending_exit = Some(n)` (1 site) |
| `take_discard() -> bool` | `take_pending_discard()` |
| `take_fatal() -> Option<i32>` | `take_pending_fatal_status()` |
| `take_exit() -> Option<i32>` | `take_pending_exit()` |
| `fatal_pending() -> bool` | `pending_fatal_status.is_some()` (14 sites) |
| `exit_pending() -> Option<i32>` | direct `shell.pending_exit` peeks (3 sites) |
| `clear_for_subshell()` | the three separate resets in `traps::clear_for_subshell` |

The atomics keep their storage and their names on `Shell`.

### 2. `pending_unwind` — one reporter, two phases

```rust
pub(crate) enum UnwindPhase {
    /// Around a command: the six `check_interrupt` sites and the wait loops.
    Around,
    /// After a command produced `Continue(c)`: `finish_command` only.
    After,
}

/// The single place that decides what stops a command, and in what order.
pub(crate) fn pending_unwind(shell: &Shell, phase: UnwindPhase) -> Option<ExecOutcome>
```

| phase | order | consulted |
|---|---|---|
| `Around` | SIGINT → timeout → exit | the atomics + `exit` |
| `After` | discard → fatal → exit | `Unwind` only |

Both arms reproduce today's behaviour exactly, including the details that look
like accidents:

- `Around` **clears** `sigint_flag` via `compare_exchange` (and returns `None`
  when SIGINT is trapped, so the trap runs instead), while leaving
  `timeout_flag` set for the builder epilogue to consume once at the run
  boundary. `exit` is peeked, not taken.
- `After`'s discard arm sets `$?` to 1 before returning (#351), and its fatal
  arm returns `Continue(c)` rather than an `Interrupted` — the fatal status is
  consumed later by the top-level reducer.

`check_interrupt` remains as a thin wrapper over `pending_unwind(shell,
Around)`, keeping its name and its six call sites untouched.

### 3. `finish_command` calls the reporter twice, on purpose

`finish_command` consults the exit request **twice**: once before the ERR trap
fires and once after, because the ERR action may itself run `exit` (v353). That
stays two calls in the `After` phase. Collapsing them would change when the ERR
trap runs relative to the unwind, which is a behaviour change.

## Migration

One signal at a time, smallest first. Sealing the slots private happens LAST,
in the task that adds the reporter — until then the struct's fields are
`pub(crate)` so each signal can move independently.

| task | moves | sites | proves itself with |
|---|---|---|---|
| 1 | `exit` into `Unwind` + accessors | ~19 / 7 files | `trap_action_exit` (28), `subshell_exit_trap` (20) |
| 2 | `discard` | ~29 / 5 files | `arith_expansion_discard`, `readonly_assign_discard` |
| 3 | `fatal` | ~65 / 11 files | the expansion + error-path harnesses |
| 4 | `pending_unwind` + `UnwindPhase`; rewire `check_interrupt` + `finish_command`; seal the slots | 2 call sites + the wrapper | full sweep |
| 5 | verification, docs, blog | — | the gate below |

Task 3 is the bulk and has no judgement in it: ~21 writes become
`raise_fatal(n)`, ~14 `is_some()` reads become `fatal_pending()`, and the
compiler enumerates the rest. It is a separate task precisely because it is
mechanical — mixing it into task 4 would bury the one task that carries
semantics.

## Verification

1. **Zero expected-value edits.** No test may change. If a task needs an
   expected value edited, the migration changed behaviour and is redone. This
   is the falsifiable claim that strict preservation buys.
2. **Full harness sweep** — `tests/scripts/run_diff_checks.sh` green at its
   current count, with both binaries built first.
3. **Tests** — `cargo test -p huck-engine --lib --jobs 1 -- --test-threads 4`
   (the 4-thread setting is required; see the fd-arc CI trap), plus the trap,
   subshell, pipeline, wait, expansion and error-path integration binaries,
   each single-threaded.
4. **bash 5.2.21 suite, PASS-set diff vs `main`** — `BASH_SOURCE_DIR=/tmp/bash-5.2.21`,
   `bash tests/bash-test-suite/runner.sh`, diffing the whole PASS set rather
   than the count. ⚠️ The runner rebuilds the RELEASE binary, so it clobbers
   `target/release/huck` with whichever branch it last ran on — rebuild
   afterwards. (This produced a stale-binary false reading during v353.)
5. **CI green before handover.** A `vNN` iteration PR is the user's to merge.

## Non-goals

- **Normalising the precedence.** The `Around`/`After` asymmetry is preserved
  and documented. If it turns out to be observable, that is a follow-up with
  its own harness evidence — not a change smuggled into a refactor.
- **Moving the atomics.** They are written from a signal handler and a timer
  thread; see Constraints.
- **Fixing bugs found on the way.** Filed, not fixed — see Constraints.
- **A sixth signal.** Nothing here anticipates future signals beyond giving
  them an obvious home.
