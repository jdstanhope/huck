# v356 — one exempt scope, propagated (including across a fork) — design

**Issues:** [#1](https://github.com/jdstanhope/huck/issues/1) — *`! eval CMD` /
`! (subshell)` does not suppress `set -e` for a failure INSIDE the body* (its
subshell third is what remains). Umbrella:
[#198](https://github.com/jdstanhope/huck/issues/198) — *unify the
error-fatality decision*.

**Follows:** v355 ([#480](https://github.com/jdstanhope/huck/issues/480)), which
split suppression into an errexit counter and an ERR-trap counter and made the
exemption reach a command's body **in-process**. This is the other half: making
it survive a fork, and collapsing the several mechanisms that answer one
question into one.

**Acceptance constraint, stated up front: the diff must be net NEGATIVE.** This
iteration replaces multiple mechanisms with one; if it adds lines it has failed,
regardless of test results. The measured target from the spike is about −18
lines of `executor.rs` + `traps.rs`.

## Problem

### The bug: the child throws the answer away

`clear_for_subshell` zeroes both suppression counters:

```rust
shell.errexit_suppressed_depth = 0;
shell.err_trap_suppressed_depth = 0;
```

A fork copies the parent's memory, so the exemption a caller established
arrives in the child intact — and is then discarded. Every exempt context is
therefore correct in-process and wrong across a fork:

```
set -e; ( false; echo x ) || echo or            bash: x     huck: or
set -e; f() { ( false; echo x ); }; f || echo or  bash: x   huck: or
set -e; ( false; echo x ) && echo and; echo after bash: x…  huck: (skips)
set -e; if ( false; echo x ); then :; fi         bash: x     huck: (nothing)
trap "echo E" ERR; ( false; echo x ) || echo or  bash: x     huck: E, x
```

Five of five fork cases wrong; the same shapes without a fork all pass since
v355. Deleting those two lines fixes all five, with `set -e; ( false )` and
`trap … ERR; ( false )` unchanged.

### The redundancy: four ways to ask one question

`finish_command`'s fire gate reads:

```rust
if c != 0 && !shell.err_trap_suppressed() && is_last && !is_negated_pipeline(cmd) {
```

`is_last` and the suppression counters answer the same question — "is this
failure being ignored?" — by different routes, because the exempt scope
currently ends *before* the epilogue runs. And `run_andor_group` still contains
two near-copies of snapshot → run → interrupt-check → control-flow propagation
→ epilogue, one for the list's first element and one for the rest.

## Design

### 1. `run_list_element` — one owner for the per-element sequence

```rust
/// Runs ONE element of an and-or list: the exempt scope, the command, the
/// interrupt checkpoint, control-flow propagation, and the post-command
/// epilogue. `Err(outcome)` means the caller must return it; `Ok(status)`
/// means carry on with the list.
fn run_list_element(
    cmd: &Command,
    exempt: bool,
    shell: &mut Shell,
) -> Result<ExecOutcome, ExecOutcome>
```

`exempt` is bash's ignore-return: an element that is **not** the syntactically
last of its list is part of a list being tested, so neither it nor anything it
runs counts. The scope therefore spans the body **and** the epilogue, which is
what lets the epilogue decide from suppression state instead of a separate
flag.

Owning both ends in one function is also what makes the scope leak-proof.
Today the raise and the lower sit either side of five early returns; get one
wrong and `set -e` silently stops working for the rest of the list. With one
exit path that failure mode cannot be written. Use a **labelled block**
(`let out = 'elem: { … };`) rather than an immediately-invoked closure — same
single-exit property without the borrow gymnastics.

`run_andor_group` becomes: run the first element, then loop the rest,
propagating `Err` immediately.

### 2. `is_last` disappears

With the scope covering the epilogue, `finish_command` loses its `is_last`
parameter and the `is_last &&` term in the fire gate. Suppression state already
carries the fact.

### 3. `clear_for_subshell` stops clearing suppression

The two lines above are deleted. What that function resets is the *trap table*
— POSIX requires a subshell to reset trapped signals to their inherited
dispositions. Suppression is not a trap; it is the caller's statement that this
command's failure does not count, and it applies to the child for exactly the
same reason it applies to a brace group.

The neighbouring resets stay: `traps.clear()`, the pending bitmask,
`firing_traps`, and `take_exit()` (v353's rule that a pending exit request
belongs to the parent).

### 4. What is deliberately NOT deleted

Two candidates were checked and kept, both because the harness proves they are
load-bearing:

- **`is_negated_pipeline`** — under `!` with errexit off, the outer command's
  ERR fire is suppressed while its body's is not (v355's contract row). Those
  are different spans, so one scope cannot express both.
- **The two counters** — same asymmetry; collapsing them reintroduces #469.
- **`body_already_fired_err`** (#445) and **`err_trap_armed`** (#444) — separate
  bash rules (fire at the innermost command; snapshot `was_error_trap` before
  the command), not restatements of exemption.

## Verification

1. **The diff must be net negative.** `git diff --stat` on the implementation
   commits, excluding tests and docs, must show more deletions than insertions.
   This is an acceptance criterion, not a preference.
2. **The exempt-scope contract** — `errexit_err_suppression_diff_check.sh`
   (32) must stay green, and gains rows for the fork family: subshell under
   `||`, `&&`, an `if` condition, a `while` condition, inside a function, and
   the ERR-trap variants. Those rows FAIL before the change and pass after.
3. **No expected-value edits** in `err_trap_compound` (30), `err_trap_function`
   (24), `set_e_andor` (34), `negated_errexit`, `trap_action_exit` (28),
   `arith_expansion_discard`. All six passed under the spike.
4. **Full sweep** green. ⚠️ Job-control harnesses are load-flaky (#476): check a
   failure against `main` by run-count before calling it a regression.
5. **bash suite PASS-set diff vs `main`**, `BASH_SOURCE_DIR=/tmp/bash-5.2.21`.
   `errexit`, `set-e` and the subshell categories are the ones that could move.
   ⚠️ Use `git checkout main` for the baseline, never `git stash` — on a clean
   tree the stash is a no-op and the "baseline" is the branch (v355 hit this);
   confirm the `huck commit:` stamp in the runner output.
6. **CI green before handover** — a `vNN` iteration PR is the user's to merge.

## Non-goals

- **A per-command ignore-return flag threaded through every signature**, the
  way bash models this internally. The spike shows the ambient scope already
  behaves correctly once it stops being discarded, and threading a parameter
  would add code — the opposite of this iteration's constraint.
- **The rest of #198.** #25/#215 (backtick syntax-error recovery) and #116
  (`history` argument error not aborting the list) are unrelated to exemption
  and stay open; the audit comment on #198 records which members remain.
- **#481** (`! ! { … }` fires twice, parity-dependent) — a parse-shape question.
- **The DEBUG/extdebug cluster** — that is the trap-inheritance generalisation,
  a separate iteration.
