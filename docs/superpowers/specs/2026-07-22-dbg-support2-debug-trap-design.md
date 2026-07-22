# v322 — DEBUG trap under extdebug: fire before assignments, `$LINENO` in the action, `return 2` skip/return

Issue: [#255](https://github.com/jdstanhope/huck/issues/255)

## Problem

The bash test-suite `dbg-support2` category exercises the DEBUG trap under
`shopt -s extdebug`. huck diverges in three independent ways; all three must
be fixed to flip the category FAIL → PASS. (`shopt -s extdebug` is already
accepted and stored; per-command DEBUG firing already works for regular
commands; the inner function-body `$LINENO` and `${FUNCNAME[1]}` already
match bash.)

### 1. DEBUG trap does not fire before a bare assignment

```sh
n=0; trap 'n=$((n+1))' DEBUG; x=1; y=2; echo "fires=$n"
```
bash `fires=3` (before `x=1`, `y=2`, `echo`); huck `fires=1` (only before
`echo`). Bare-assignment commands dispatch via `SimpleCommand::Assign` →
`run_assignment_list`, which never calls `fire_debug_trap` (that call lives in
the Exec path, `run_exec_single_inner`).

### 2. `$LINENO` inside the DEBUG action is stuck at 1

```sh
trap 'echo "L=$LINENO"' DEBUG
echo first    # bash L=2 ; huck L=1
echo second   # bash L=3 ; huck L=1
```
bash makes the DEBUG action's top-level `$LINENO` equal the line of the
command about to execute. huck runs the action via `process_line`, which
re-parses the action string as its own line-1 script and stamps
`current_lineno = line_base() + 1`, clobbering it to 1.

### 3. extdebug: a non-zero DEBUG-trap status skips the command; status 2 in a subroutine simulates `return`

Verified semantics against bash 5.2.21 (`shopt -s extdebug` ON; the DEBUG
action's *exit status* is what matters — in `dbg-support2` it is produced by a
helper function returning 0 or 2, since a bare `return` in the action itself
is a top-level error):

- DEBUG action exit status **non-zero** → **skip** the command about to run
  (the next command is not executed). rc = 1, 2, 3 all skip.
- DEBUG action exit status **exactly 2** **and** the shell is executing in a
  subroutine (a function or a sourced script) → instead of skipping one
  command, **simulate a `return`** from that subroutine (return code 2 —
  `f; echo $?` prints 2; the rest of the function body is abandoned). rc = 1
  or 3 in a function skip ONE command and the function continues (return 0).
- extdebug **off** → a non-zero DEBUG status is ignored (no skip).

`dbg-support2` triggers status 2 at **top level**, where it just skips the one
command (`x=2`), so the second `echo` still sees `x` == 1. The full semantics
(the in-subroutine `return` simulation) are in scope per the design decision.

## Design

### `extdebug()` accessor (shell_state.rs)

Add, alongside `extglob()`:

```rust
/// True when `shopt -s extdebug` is in effect.
pub fn extdebug(&self) -> bool {
    self.shopt_options.get("extdebug").unwrap_or(false)
}
```

### `fire_debug_trap` returns a decision (traps.rs)

`fire_pseudo_trap` currently discards the action's status
(`let _ = process_line(...)`). Split the DEBUG firing out so it can (a) reframe
`$LINENO` for the action and (b) inspect the action's exit status.

```rust
/// What the caller must do after the DEBUG trap action ran.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DebugDecision {
    /// Run the pending command normally.
    Proceed,
    /// extdebug + non-zero DEBUG status: skip the pending command.
    SkipCommand,
    /// extdebug + status 2 in a subroutine: simulate `return n`.
    ReturnFromSub(i32),
}
```

`fire_debug_trap(shell) -> DebugDecision`:

1. Recursion guard (unchanged): if `shell.firing_trap == Some(Debug)`, return
   `Proceed` (the action's own commands must not re-fire / re-skip).
2. Look up the DEBUG action; if none, return `Proceed`.
3. **Reframe `$LINENO` for the action.** `shell.current_lineno` was stamped to
   the pending command's line by the dispatch site *before* this call. Save
   the current `eval_frame`, set `eval_frame = Some(current_lineno)`, run the
   action via `process_line`, then restore `eval_frame`. This reuses the v315
   `line_base()` machinery — `line_base() = eval_frame.saturating_sub(1)`, and
   each command stamps `current_lineno = line_base() + cmd.line`, so the
   action's line-1 command resolves to `(current_lineno - 1) + 1 =
   current_lineno` (the same mechanism that made `$LINENO` correct inside
   `eval`). Skip the reframe when `current_lineno == 0` (synthesized commands).
4. Read the action's exit status from `shell.last_status()` (`$?`).
5. Decide:
   - `!shell.extdebug()` or `status == 0` → `Proceed`. (`status` is
     `shell.last_status()`.)
   - `status == 2 && in_subroutine(shell)` → `ReturnFromSub(2)`.
   - otherwise (extdebug + non-zero) → `SkipCommand`.

where `in_subroutine(shell) = !shell.call_stack.is_empty() || shell.source_depth > 0`.

`RETURN`/`ERR`/real-signal traps keep the existing `fire_pseudo_trap` path
(status-agnostic); only DEBUG needs the decision. `fire_debug_trap`'s two
existing unit tests are updated to the new return type (they assert firing +
recursion-guard behavior; both now also assert `Proceed`).

### Dispatch sites honor the decision (executor.rs)

Both leaf-command dispatch sites already stamp `$LINENO` immediately before
firing; they now branch on the returned `DebugDecision`.

**Exec path** — `run_exec_single_inner`, at the existing `fire_debug_trap`
call (currently line ~4257, return value ignored):

```rust
match crate::traps::fire_debug_trap(shell) {
    DebugDecision::Proceed => {}
    DebugDecision::SkipCommand => return ExecOutcome::Continue(shell.last_status()),
    DebugDecision::ReturnFromSub(n) => return ExecOutcome::FunctionReturn(n),
}
```

**Assign path** — `run_simple` `SimpleCommand::Assign(items, line)` arm
(executor.rs:3865), which currently stamps `$LINENO` and calls
`run_assignment_list` with no DEBUG fire. Add the fire + branch *after* the
LINENO stamp and *before* `run_assignment_list`:

```rust
if *line != 0 {
    shell.current_lineno = shell.line_base() + *line;
}
match crate::traps::fire_debug_trap(shell) {
    DebugDecision::Proceed => {
        let procsub_base = shell.procsub_pending.len();
        let st = run_assignment_list(items, shell, sink, err_sink);
        drain_procsubs(shell, procsub_base);
        ExecOutcome::Continue(st)
    }
    DebugDecision::SkipCommand => ExecOutcome::Continue(shell.last_status()),
    DebugDecision::ReturnFromSub(n) => ExecOutcome::FunctionReturn(n),
}
```

Skipping means the assignment's RHS is never expanded (no procsub realized),
so no drain is needed on that branch. `$?` is left at its prior value
(`Continue(shell.last_status())` — the skipped command contributes nothing).

**Why these two sites suffice.** These are the leaf simple-command dispatch
points; every command bash fires DEBUG before is one of them (compound
commands fire DEBUG on their constituent simple commands, which route here).
`FunctionReturn(n)` is the existing signal that `call_function` /
`run_sequence` already propagate to unwind a function or sourced frame, so the
in-subroutine `return 2` simulation reuses that machinery with no new
unwinding path. At top level there is no frame, so `in_subroutine` is false
and status 2 becomes `SkipCommand` (bash cannot `return` at top level either).

### Ordering note

The DEBUG fire must stay *after* the `$LINENO` stamp (so the action sees the
pending command's line) and *before* argument/RHS expansion and execution (so
a skip prevents side effects). Both sites already stamp first; the fire slots
in right after.

## Testing

Gate = bash 5.2.21 fidelity.

1. **Bash-diff harness — gold standard.** Add
   `tests/scripts/debug_trap_extdebug_diff_check.sh` (model on an existing
   `*_diff_check.sh`; `check "label" 'fragment'` comparing `bash --norc
   --noprofile` vs `$HUCK_BIN`, byte-identical incl. exit). Synthetic
   fragments (NOT bash's GPL `dbg-support2.tests`):
   - DEBUG fires before bare assignments: `n=0; trap 'n=$((n+1))' DEBUG;
     x=1; y=2; echo $n` → `3`.
   - `$LINENO` in the action tracks the pending command (multi-line script).
   - extdebug + helper-returns-2 at top level skips the next command
     (`x=1; de=2; x=2; echo $x` → `1`).
   - extdebug + helper-returns-1/3 skips one command (any non-zero).
   - extdebug + helper-returns-2 inside a function simulates return
     (`f(){ echo A; de=2; echo B; echo C; }; f; echo done` → `A`,`done`;
     `echo $?` after → 2).
   - extdebug + returns-1/3 inside a function skips ONE command, function
     continues (→ `A`,`C`, return 0).
   - extdebug **off**: non-zero DEBUG status does NOT skip.
   - `${FUNCNAME[1]}` == `main` from a top-level DEBUG action (regression).
   Auto-discovered by `run_diff_checks.sh`'s `*_diff_check.sh` glob.

2. **Unit tests.** `traps.rs`: `fire_debug_trap` returns `SkipCommand` when
   extdebug + a DEBUG action exits non-zero; `ReturnFromSub(2)` when status 2
   + a pushed `call_stack` frame; `Proceed` when extdebug off / status 0 /
   recursion-guarded. Keep them at the traps layer (drive `Shell` directly,
   install a DEBUG action, set `shopt_options`/`call_stack`, assert the
   decision) — no parser dependency.

3. **`dbg-support2` category flip.** Run the runner
   (`HUCK_BASH_TEST_CATEGORY=dbg-support2 BASH_SOURCE_DIR=<bash-src>`) and
   confirm `dbg-support2 | PASS`, empty diff. Update
   `docs/bash-test-suite-baseline.md` (PASS 18 → 19, FAIL 64 → 63; move
   `dbg-support2` to PASS; the row at :150 currently blames the wrong root —
   "LINENO inside functions" — replace it).

4. **Regression guards.** `$LINENO` outside any trap unchanged; regular
   per-command DEBUG firing unchanged; a DEBUG trap with no extdebug and a
   non-zero action does not skip; `bash_source_lineno` / `lineno` / `trap`
   integration binaries green; full `run_diff_checks.sh` sweep green;
   huck-engine + huck-syntax lib suites green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard bash-diff sweeps with `ulimit -v 1500000` + `timeout`;
run the `-p huck` trap/lineno integration binaries single-threaded before push.

## Scope

**In scope.** `extdebug()` accessor; `DebugDecision` + `fire_debug_trap`
return; the `$LINENO` action reframe; both dispatch-site branches; the
harness + unit tests + baseline flip.

**Out of scope.** Any other DEBUG/extdebug feature not needed for the flip:
`BASH_ARGC`/`BASH_ARGV`, `declare -F` line/file info, `trap -p DEBUG`
formatting, the `extdebug` `debugger`-profile behaviors. The DEBUG trap firing
granularity for compound-command internals is already correct and unchanged.

## Documentation

- `docs/bash-test-suite-baseline.md`: PASS 18 → 19; `dbg-support2` → PASS;
  fix the stale root-cause note.
- `docs/architecture.md`: if traps / `$LINENO` are described, note the DEBUG
  action's line reframe and the extdebug skip/return decision.
- Removes a divergence (no new intentional one); #255 auto-closes via the PR
  body (`Closes #255`).
