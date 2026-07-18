# v312 — a `$(( ))` arithmetic expansion error discards the current command

**Issues:** [#3](https://github.com/jdstanhope/huck/issues/3) (a failed arithmetic
expansion returns rc 0 and still runs the command) and, by the same root,
[#49](https://github.com/jdstanhope/huck/issues/49) (arithmetic errors in `-c`
mode continue instead of halting). Second member of the error-fatality funnel
umbrella [#198](https://github.com/jdstanhope/huck/issues/198).

**Goal:** an arithmetic *expansion* error (`$(( ))`, including in an assignment
RHS or an array subscript) discards the current top-level command — the command
does not run, the rest of that command (including out of loops and functions) is
abandoned, and the status is 1 — but the shell does **not** exit, matching bash.

---

## The measured bash model (this is subtler than the issue states)

`set -e` is NOT involved; all rows are plain non-interactive bash 5.2.21.

| input | bash |
|---|---|
| `echo $((3.5)); echo done` (`-c`) | error; `echo` does not run, `done` does not run; **rc 1** |
| `echo BEFORE; echo $((3.5)); echo AFTER` (`-c`) | `BEFORE`; then error; AFTER not run; rc 1 |
| multi-line script: `echo $((3.5))` / `echo L2` / `echo L3` | error on line 1; **L2 and L3 DO run**; rc 0 |
| `for i in 1 2 3; do echo i$i; echo $((3.5)); echo t$i; done; echo END` | prints only `i1`, then aborts the whole loop AND `END`; rc 1 |
| `f(){ echo in; echo $((3.5)); echo after_in; }; f; echo AFTER_F` | prints `in`; then aborts out of `f` AND `AFTER_F`; rc 1 |
| `x=$( echo $((3.5)) ); echo "[$x] after"` | comsub captures empty; **`after` DOES run**; rc 0 |
| `x=$((3.5)); echo AFTER` | assignment discarded; AFTER not run; rc 1 |
| `a[$((3.5))]=1; echo AFTER` | discarded; AFTER not run; rc 1 |
| `(( 3.5 )); echo done` / `for ((i=3.5;;))` / `let "3.5"` | **non-fatal** (already correct in huck) — different path |

**Two conclusions:**
1. An arithmetic expansion error is bash's `jump_to_top_level(DISCARD)`: it unwinds
   the *current top-level command* (out of loops and functions) but does **not**
   exit the shell — the next top-level command (next script line) runs. This is
   **distinct** from `set -u`/`${x?}`, which do `jump_to_top_level(EXITPROG)` and
   *exit* the non-interactive shell (a multi-line script's later lines do NOT run —
   verified). The funnel (#198) has two fatality flavors; this is the second.
2. A command substitution `$( … )` is an execution **boundary** that *contains*
   the discard (empty capture, status 1) — the outer command continues. Same
   containment as a `! ( subshell )` (cf. v311). The arithmetic *command* `(( ))`,
   `for ((;;))`, and the `let` builtin are non-fatal and use a different code path
   (`run_arith`/`eval_arith_word`, not the word-expansion site) — untouched.

huck today: swallows the error — expands `$(( ))` to empty, runs the command,
returns rc 0 (`expand.rs:1206`/`:1817` emit the diagnostic then call
`posix_fatal(127)`, a no-op outside POSIX mode). Exit code should be **1** (bash
uses 1 for arithmetic expansion errors, not 127).

## Design (Approach A — a discard-current-command unwind)

huck already has an unwind channel that propagates like `Exit` up through loops,
functions, and-or lists, and the sequence body, then is decoded at a top-level
consumer: `ExecOutcome::Interrupted(InterruptReason)` (used for SIGINT/timeout).
The ~15 intermediate propagation sites match it reason-generically
(`Interrupted(_)` / `Interrupted(r) => return …`), so a **new reason** rides that
channel for free; only the boundary/decoder sites change. This is Approach A
(the user-approved discard unwind), implemented via the existing channel rather
than a brand-new `ExecOutcome` variant — far less surface and risk.

**Components:**

1. **`InterruptReason::FatalExpansion`** (new variant, `builtins.rs`). Documented
   as a *synchronous* discard (bash `jump_to_top_level(DISCARD)`), not a signal —
   it shares the `Interrupted` unwind mechanism but is decoded differently.

2. **`Shell::pending_discard: bool`** (new, `shell_state.rs`), mirroring
   `pending_fatal_status` but for the discard flavor, with a `take_pending_discard`
   accessor.

3. **Trigger** (`expand.rs:1206` and `:1817`, the two `$(( ))` word-expansion
   error sites): after emitting the diagnostic, set `shell.pending_discard = true`
   (replacing the `posix_fatal(127)` call). The empty contribution stays
   (harmless — the command is discarded before it runs).

4. **Conversion**: at the executor points that already convert
   `pending_fatal_status` after expansion (`executor.rs:436`, `:479`, and the
   assignment/subscript/command-word paths at `:2316`/`:3439`/`:3484`), also check
   `pending_discard`; if set, `take` it and return
   `ExecOutcome::Interrupted(InterruptReason::FatalExpansion)`. (Keep the
   `pending_fatal_status` checks exactly as-is — the two flags are independent.)

5. **Propagation**: free via the existing reason-generic `Interrupted` sites —
   unwinds loops (`run_for_inner`/`run_while_inner`), function calls, and-or lists,
   and the sequence body.

6. **Containment at the in-process comsub boundary** (`execute_capturing`,
   `executor.rs:372`): add a `FatalExpansion` arm that returns status **1**
   **without** re-raising any flag (the comsub is contained; the outer command
   continues). Contrast the existing `Sigint`/`Timeout` arms, which re-raise so the
   enclosing list aborts. (Fork-based boundaries — a `( subshell )` or `&`
   background — contain it automatically: the child unwinds and `_exit`s with
   status 1.)

7. **Driver** (`run_sourced_contents_in_sinks`, `builtins.rs:~182`): the `'outer`
   loop currently does `Interrupted(r) => return Interrupted(r)`. Change to: for
   `FatalExpansion`, set `last_status = 1` and **continue** the loop (read the next
   top-level unit — do NOT return/exit); `Sigint`/`Timeout` keep returning. This is
   what makes `-c 'A; B'` abort B (one unit, no next) but a multi-line script
   continue (line 2 is the next unit).

8. **Reducers** (defensive, in case it escapes the driver loop):
   `executor.rs:~8058` and `shell.rs:294`'s `Interrupted` decode add
   `FatalExpansion => 1`.

**Status is 1** everywhere (bash's arithmetic-expansion rc). No `set -e`
interaction is special-cased — a discarded command with rc 1 flows through the
normal errexit gate like any other rc-1 command.

## Scope boundaries

- **Only `$(( ))` arithmetic *expansion*** (command word, assignment RHS, array
  subscript). The `(( ))` arithmetic command, C-style `for ((;;))`, and `let` are
  non-fatal and route through `run_arith`/`eval_arith_word` (a different path) —
  **not touched**; the harness pins them staying non-fatal.
- **The error-message wording is OUT OF SCOPE.** huck says
  `3.5: unexpected character: '.' (error token is "")`; bash says
  `3.5: syntax error: invalid arithmetic operator (error token is ".5")`. That is
  an arithmetic-diagnostic gap (part of #60), not this fix. The harness compares
  the *abort behavior* (which commands run) and the *rc*, normalizing away the
  error-message line.
- **POSIX mode**: replacing `posix_fatal(127)` changes POSIX+non-interactive from
  exit-127 to discard-rc-1. Verify against `bash --posix` in the plan; if bash
  POSIX mode genuinely *exits* on an arithmetic error, keep a POSIX branch that
  sets `pending_fatal_status` instead. (Default, non-POSIX mode is the #3/#49
  target and takes the discard path unconditionally.)

## Testing

New `tests/scripts/arith_expansion_discard_diff_check.sh`, byte-diffing huck vs
bash on **which commands run and the rc**, with the error-message line normalized
(`sed` the `arith.*error`/`unexpected character` diagnostic to a fixed token so
only fatality/ordering is compared). Cover every measured row above:
- discard cases (`-c` `A; B`, BEFORE/AFTER, assignment, subscript, division `1/0`),
- the multi-line-script-continues case (a real temp-file script: L2/L3 run),
- the loop-unwind and function-unwind cases,
- the comsub-containment case (`x=$( echo $((3.5)) ); echo after` → `after` runs),
- controls that must stay non-fatal (`(( 3.5 ))`, `for ((i=3.5;;))`, `let "3.5"`,
  and a valid `$((1+1))`),
- a `set -e` row (a discarded rc-1 command under `set -e` aborts like any rc-1).

Plus: the existing arithmetic and `set -e` harnesses and the engine lib tests must
stay green; run `tools/redirect_audit.sh` is NOT needed (no fd change), but the
full `run_diff_checks.sh` sweep must be green.

## Rejected alternatives

- **Reuse `pending_fatal_status(1)`** (the exit-shell flavor). Simplest, and fixes
  the `-c` repro, but it *exits* the shell on a multi-line script and after a
  loop/function where bash continues — trading #3 for a worse divergence.
- **A brand-new `ExecOutcome::DiscardCommand` variant.** Cleaner semantics but
  requires touching ~22 outcome match arms; reusing the reason-generic
  `Interrupted` channel gets the same unwind with only the boundary/decoder sites
  changed.
- **Per-command abort only.** Fails `echo BEFORE; echo $((3.5)); echo AFTER` (AFTER
  must be discarded) and the loop/function unwind.
