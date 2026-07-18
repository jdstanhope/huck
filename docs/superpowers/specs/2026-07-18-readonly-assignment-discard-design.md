# v313 — a readonly-variable assignment error discards the current command

**Issue:** [#31](https://github.com/jdstanhope/huck/issues/31) — a readonly
assignment does not abort a non-interactive shell (default mode). Third member of
the error-fatality funnel umbrella [#198](https://github.com/jdstanhope/huck/issues/198).

**Goal:** `readonly r=1; r=2; echo done` (and `UID=5; echo done`) prints the error
and discards the current command (rc 1) — `done` does not run — matching bash's
default non-interactive mode, without exiting the shell.

---

## The measured bash model (the issue's "EXITS" framing is wrong)

The issue says bash "EXITS immediately" on a readonly-assignment error. Measured
against bash 5.2.21, it does not *exit* in default mode — it **discards the
current top-level command**, exactly like the arithmetic-expansion error fixed in
v312 (#3). It is the **DISCARD** flavor, not **EXITPROG**.

| input (default mode, non-interactive) | bash |
|---|---|
| `readonly r=1; r=2; echo done` (`-c`) | error; `done` not run; **rc 1** |
| multi-line script: `readonly r=1` / `r=2` / `echo L2` / `echo L3` | error; **L2 and L3 DO run**; rc 0 (shell NOT exited) |
| `echo B; r=1; readonly r; r=2; echo A` | `B`; then error; `A` not run |
| `readonly r=1; for i in 1 2 3; do echo i$i; r=2; echo t$i; done; echo END` | prints only `i1`, aborts the whole loop AND `END`; rc 1 |
| `readonly r=1; f(){ echo in; r=2; echo after_in; }; f; echo AF` | prints only `in`, unwinds out of `f` AND `AF`; rc 1 |
| `readonly r=1; a=1 r=2 b=3; echo x` (assignment LIST) | discards the whole command; `echo x` not run; rc 1 |
| **`--posix` mode**, same | **exits** (rc 1) — already handled by v226's `posix_fatal` |

**Non-fatal cases that already match bash and must stay untouched:**
- Inline-prefix assignment `r=2 cmd` — reported but non-fatal; the shell continues.
- `unset` of a readonly variable — non-fatal.
- `for r in …` / `select r in …` where the loop *variable* is readonly — bash
  skips the loop and continues (`END` runs); huck already matches.

So a **standalone assignment statement** (`r=2`, or an assignment list
`a=1 r=2 b=3`) whose target is readonly is the DISCARD case. This is the same
fatality flavor as #3 — v312 already built the mechanism.

huck today: prints `NAME: readonly variable`, sets status 1, but **continues** —
`run_assignment_list` (`executor.rs:4105`) reports the readonly error and returns
`Continue(1)` (only `posix_fatal(127)` fires, a no-op outside POSIX).

## Design

Two parts: a small generalizing rename, then the one-site reuse of v312's DISCARD
mechanism.

### Part 1 — rename `InterruptReason::FatalExpansion` → `DiscardCommand`

v312 introduced the DISCARD mechanism named for its first user (arithmetic
*expansion*). #31 is a second, non-expansion user (a readonly *assignment*), so
the name is now inaccurate. Rename `InterruptReason::FatalExpansion` →
`InterruptReason::DiscardCommand` across all ~15 occurrences (`builtins.rs`
enum + driver arm, `shell_state.rs`, `shell.rs` reducer, `expand.rs` comment
refs, `executor.rs` comsub/fork/backstop arms, `repl.rs` arm). The `pending_discard`
flag name is already general and stays. Pure mechanical rename — no behavior
change; the v312 arith tests must stay green.

Update the doc comment on the variant to describe it generally: "a fatal error
that DISCARDS the current top-level command (bash `jump_to_top_level(DISCARD)`) —
unwind out of loops/functions, status 1, shell NOT exited. Raised by a fatal
`$(( ))` expansion error (#3) and a readonly-variable assignment error (#31);
contained at execution boundaries; the driver loop continues on it."

### Part 2 — route the readonly-assignment error through DISCARD

In `run_assignment_list` (`executor.rs:4105`), at the readonly-error handling
(the `posix_fatal(127)` sites ~`4123`/`4128` and the nameref-resolved-target site
~`4117`), replace the bare `shell.posix_fatal(127)` with the v312 pattern:

```rust
    if shell.shell_options.posix && !shell.is_interactive {
        shell.posix_fatal(127);        // EXITPROG (v226): POSIX non-interactive exits 127
    } else {
        shell.pending_discard = true;  // DISCARD (v312/#3): discard the current command, rc 1
    }
```

The existing `pending_discard` machinery converts it (at the same post-command
`run_andor_group` `Continue`-backstop that already handles the flag) to
`ExecOutcome::Interrupted(DiscardCommand)`, which the reason-generic unwind
channel propagates through loops/functions/lists, contained at the comsub/fork
boundaries, and the driver loop continues on (no shell exit), decoding to rc 1.

**Interactive** takes the discard `else` branch (the command is discarded, the
REPL reprompts with `$?`=1) — matching bash, which never exits interactively on a
readonly error. **POSIX+non-interactive** keeps the exit-127 behavior (v226,
unchanged).

`run_assignment_list` currently `break`s / `return`s `Continue(1)` after the
error. Keep returning `Continue(1)` — the flag is converted to the unwind by the
enclosing backstop exactly as `$(( ))` is; do NOT return `Interrupted` directly
here (that would bypass the backstop's established conversion point and diverge
from the v312 path).

## Scope boundaries

- **Only standalone assignment statements** (`run_assignment_list`): `r=2`,
  `a=1 r=2 b=3`. NOT the for-loop/select variable binds (`executor.rs:1828`,
  `:2237` — already match bash), NOT inline-prefix `r=2 cmd`, NOT `unset`.
- **The error-message wording is unchanged** (huck already matches bash for the
  readonly message: `NAME: readonly variable`). No normalization needed, but the
  harness compares stdout+rc primarily.
- **Other assignment-error kinds** (invalid identifier, bad subscript) are NOT in
  scope — this fix is the readonly case (#31). If bash treats those as DISCARD
  too, that is a separate follow-on.

## Testing

New `tests/scripts/readonly_assign_discard_diff_check.sh`, byte-diffing huck vs
bash (stdout + rc; stderr compared too — the readonly message already matches):
- **Fix (red→green):** `readonly r=1; r=2; echo done`; `UID=5; echo done`;
  `BASH_VERSINFO[0]=9; echo done`; the assignment-list `a=1 r=2 b=3; echo x`; the
  before/after case; the loop-unwind and function-unwind cases.
- **Multi-line SCRIPT (must NOT exit):** a temp-file script where line-2 readonly
  error still runs L2/L3 (rc 0).
- **Controls (stay non-fatal / already-correct):** inline-prefix `r=2 echo RAN`
  (RAN + continue), `unset` of a readonly (continue), `for r in a b` with r
  readonly (loop skipped, `END` runs), a normal successful assignment.
- **POSIX:** `bash --posix` vs `huck --posix` on `readonly r=1; r=2; echo done`
  (both exit, no `done`) — the v226 EXITPROG path must be preserved.

Plus: the v312 arith harness (`arith_expansion_discard_diff_check.sh`) and the
`readonly`/assignment lib tests must stay green (the rename must not disturb
them); full `run_diff_checks.sh` sweep green.

## Rejected alternatives

- **Treat it as EXITPROG (`pending_fatal_status`).** The issue's framing, and
  what a naive read suggests — but measured bash *does not exit* in default mode
  (multi-line scripts continue). EXITPROG would exit where bash discards.
- **Return `Interrupted(DiscardCommand)` directly from `run_assignment_list`.**
  Bypasses the established backstop conversion point; setting `pending_discard`
  and letting the backstop convert keeps #31 on the identical path as #3.
