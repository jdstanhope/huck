# huck v138 — SIGINT (Ctrl-C) aborts the running command list Design

**Status:** approved design, ready for implementation plan.
**Implements:** make an untrapped Ctrl-C abort the currently-running command
list / function / script like bash — interactive returns to a fresh prompt with
`$?`=130 (the shell does NOT exit); `-c`/script exits 130. A user `INT` trap still
runs its handler and continues; `trap '' INT` still ignores. This is a Tier-1 bug
(huck currently runs the rest of the list to completion on Ctrl-C).
**Branch (impl):** `v138-sigint-abort`.

## Background — measured root cause

huck honors a pending SIGINT only inside loop bodies (`run_for`/`run_while`/
`run_until`/C-for) and in `wait`/`read`. It does NOT abort between commands in a
sequence: `run_andor_group` / `execute_sequence_body` treat a `Continue(130)` like
any ordinary status and run the next command; there is no propagation across
sequence, function-call, or command-substitution boundaries. And the loop CLEARS
the flag on read (`sigint_flag.compare_exchange(true, false, …)`), so even when a
loop does abort with `Continue(130)`, the enclosing sequence neither sees the flag
nor recognizes the 130, and keeps going.

`nvm ls` builds its output from sequences of commands, function calls, and command
substitutions (not one long polling loop), so a Ctrl-C is never observed and it
runs to completion.

Measured (debug binary), deterministic via `kill -INT $$` (sets huck's own
`sigint_flag`), run from a script file:

```
echo a; kill -INT $$; echo b              bash: rc 130, prints "a"      huck: rc 0, prints "a b"   (BUG)
for i in 1 2 3; do echo $i; kill -INT $$; done; echo after
                                          bash: rc 130, prints "1"      huck: rc 0, prints "1 after" (BUG)
trap 'echo c' INT; echo a; kill -INT $$; echo b
                                          bash: rc 0, "a c b"           huck: rc 0, "a c b"        (already correct)
trap '' INT;       echo a; kill -INT $$; echo b
                                          bash: rc 0, "a b"             huck: rc 0, "a b"          (already correct)
```

The loop case proves the gap is at the SEQUENCE level: huck's loop returns
`Continue(130)` but `echo after` still runs. bash unwinds the whole running list.

**Job-control subtlety (the interactive external-command case).** In an
interactive shell running a FOREGROUND external command, huck `tcsetpgrp`s the
terminal to the child's process group, so a terminal Ctrl-C is delivered to the
CHILD's pgroup — the child dies from SIGINT but huck itself does NOT receive the
signal, so `sigint_flag` is never set. The trigger there is the child terminating
via SIGINT (`WIFSIGNALED && WTERMSIG==SIGINT`). For huck's own in-process work
(functions/sequences/loops) and for ALL non-interactive modes (`-c`/script share
the foreground pgroup), huck DOES receive SIGINT and `sigint_flag` is set.

## Architecture — a propagating `Interrupted` outcome

### Change 1 — new control-flow outcome `ExecOutcome::Interrupted`

Add a 5th variant to `ExecOutcome` (builtins.rs:12):

```rust
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak(u32, i32),
    LoopContinue(u32),
    FunctionReturn(i32),
    Interrupted,           // v138: untrapped SIGINT — abort the running list
}
```

`Interrupted` propagates exactly like `Exit` at every intermediate level (it is
added to every "propagate immediately" `matches!` set and every exhaustive `match
ExecOutcome`), so it bubbles through sequences, and-or groups, loops, `if`/`case`,
functions, and command substitutions until a top-level consumer handles it. The
Rust compiler will flag every exhaustive `match` that needs the new arm; the
non-exhaustive `matches!(… Exit(_) | LoopBreak | LoopContinue | FunctionReturn)`
propagation sites are found by grep and each gets `| ExecOutcome::Interrupted`.

Rationale vs. a sticky-flag approach: huck already handles four propagating
control-flow outcomes uniformly through these `matches!` checks; a 5th variant
rides that infrastructure, is type-safe, and is compiler-guided. A parallel
flag-check mechanism would duplicate logic at every site, be ambiguous with a real
`exit 130`, and be fragile about where to clear.

### Change 2 — `check_interrupt` helper (the trigger + trap gating)

A single chokepoint, e.g. in `executor.rs`:

```rust
/// Returns Some(Interrupted) when an untrapped SIGINT is pending and should
/// abort the running list. Consumes the pending flag. Returns None when there is
/// no pending SIGINT, OR when a user INT trap (handler or ignore-form) is
/// installed — in which case the existing trap machinery handles it and execution
/// continues (matching bash).
fn check_interrupt(shell: &Shell) -> Option<ExecOutcome> {
    use std::sync::atomic::Ordering;
    if shell.sigint_flag
        .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        // A user INT trap (handler `trap 'cmd' INT` OR ignore `trap '' INT`) is
        // tracked in shell.trap_sigids/traps; let the existing trap_pending
        // dispatch run the handler (or no-op the ignore) and do NOT abort.
        if shell.trap_sigids.contains_key(&libc::SIGINT) {
            return None;
        }
        return Some(ExecOutcome::Interrupted);
    }
    None
}
```

`shell.trap_sigids` holds a `SIGINT` key for both the handler form and the
ignore-form (see `traps::install`), so the gate correctly suppresses the abort in
both cases. This is the SIGINT analogue of v137's PIPE-trap respect.

**Trigger (b): foreground child killed by SIGINT.** At the foreground-wait sites
that already compute `128 + WTERMSIG(raw_status)` for a signaled child
(executor.rs:424-425, 445-446, 3461-3462, and the pipeline wait at 4275 /
4299-4300), when `WIFSIGNALED(raw_status) && WTERMSIG(raw_status) == libc::SIGINT`,
set `shell.sigint_flag` (store true) so the subsequent `check_interrupt` aborts.
This covers the interactive job-control case where huck did not receive the signal
directly. (The status the command reports stays 130, as today.)

### Change 3 — propagation points

Add `Interrupted` propagation at:
- **Loops** `run_for`/`run_for_arith`(C-for)/`run_while`/`run_until`: replace the
  existing `compare_exchange`-and-return-`Continue(130)` with `if let Some(o) =
  check_interrupt(shell) { return o; }`, and add `Interrupted` to each loop's
  body-outcome propagation `match`.
- **`run_andor_group`**: after `run_command` for the first command AND each
  `rest` command, `if let Some(o) = check_interrupt(shell) { return o; }`
  (placed alongside the existing control-flow propagation `matches!`, which also
  gains `| Interrupted`). This is the key new site — it catches a SIGINT that
  arrived during a simple/external command (which returns `Continue(130)` by
  signal death, not by self-checking).
- **`execute_sequence_body`**: add `Interrupted` to the between-group propagation
  `matches!` (it already returns early on `Exit`/`LoopBreak`/…).
- **`run_if` / `run_case` / `run_pipeline`**: add `Interrupted` to their
  propagation `matches!` so it bubbles out of conditions/branches/pipelines.
- **`call_function` / `call_function_body`**: propagate `Interrupted` from the
  body (do not convert it to `FunctionReturn`/`Continue`); the enclosing
  `run_andor_group` then re-checks and continues unwinding.
- **Command substitution** (`run_substitution` / `execute_capturing`): the forked
  child aborts its body on SIGINT (it inherits the handler; its own
  `check_interrupt` raises `Interrupted`, ending the child). The parent, which
  also received the group SIGINT (or whose child died of it), has `sigint_flag`
  set, so the enclosing `run_andor_group`'s `check_interrupt` aborts after the
  consuming simple command returns. `execute_capturing` maps an `Interrupted`
  body outcome to a best-effort partial capture + propagates upward.

### Change 4 — top-level consumption

- **Interactive REPL** (`shell.rs` run loop, which calls `process_line` →
  `ExecOutcome`): on `Interrupted`, set `$?`=130, print a newline to the terminal,
  and **continue the loop (reprompt) — do NOT exit**. The flag was already cleared
  by `check_interrupt`.
- **`-c` / script** (`run_program` / `run_sourced_contents`): on `Interrupted`,
  stop executing further top-level units and return exit code 130. (`run_program`'s
  `match outcome` and the `run_sourced_contents` per-unit loop each gain an
  `Interrupted => 130 / break` arm.)

### Status
`$?`=130 (128 + SIGINT). The script/`-c` process exit code is 130.

## Scope & must-not-regress
- **`trap 'cmd' INT`** runs the handler and execution CONTINUES (no abort) — the
  `check_interrupt` gate returns `None` when an INT trap exists; the existing
  `dispatch_pending_traps` runs the handler. Currently correct; must stay.
- **`trap '' INT`** ignores SIGINT (no abort, no handler) — same gate. Currently
  correct; must stay.
- **A command that legitimately exits 130** (not via SIGINT) must NOT abort the
  list — the abort is driven by the SIGINT FLAG, never by the 130 status value.
- **Loops' `break`/`continue`/`return`/`exit` semantics** unchanged — `Interrupted`
  is a distinct outcome added beside them.
- **Job-control PTY behavior** (Ctrl-Z stop, subshell tty hand-off, background
  jobs) unaffected — v138 only adds an abort path for SIGINT.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/builtins.rs` | Add `ExecOutcome::Interrupted`. Update exhaustive `match ExecOutcome` arms the compiler flags (e.g. in `wait`/`read` self-check sites, which now return `Interrupted` instead of `Continue(130)` when untrapped). |
| `src/executor.rs` | Add `check_interrupt`; call it in loops + `run_andor_group`; set `sigint_flag` at the 5 foreground-wait `WTERMSIG==SIGINT` sites; add `Interrupted` to every propagation `matches!`/`match` (loops, `run_if`, `run_case`, `run_pipeline`, `execute_sequence_body`, `call_function(_body)`, command-sub). |
| `src/shell.rs` | REPL: handle `Interrupted` from `process_line` → `$?`=130, newline, reprompt (no exit). `run_program`: `Interrupted => 130`. |
| `src/builtins.rs` (`run_sourced_contents`) | Per-unit loop: on `Interrupted`, stop and return 130. |
| `tests/sigint_abort_integration.rs` (NEW) | Deterministic `kill -INT $$` vectors (sequence, loop, function body, nested, command-sub) abort with rc 130 and the truncated output; trap-handler + trap-ignore keep running; a legit `exit 130`/`return 130` does NOT abort. |
| `tests/scripts/sigint_abort_diff_check.sh` (NEW, 58th) | Byte-identical bash↔huck over the `kill -INT $$` vectors (run as file-args; stdout+rc compared). |
| `tests/sigint_abort_pty.rs` (NEW, mirrors existing `*_pty.rs`) | Real Ctrl-C during a foreground external (`sleep`) and a shell-function loop → returns to a fresh prompt, `$?`=130, shell alive (trigger (b)). Skips gracefully without a PTY. |
| `docs/bash-divergences.md` | Reopen Tier-1 0→1 during work; DELETE on merge (this IS the fix). No standing Tier-1 entry remains. |

## Testing

1. **Deterministic integration `#[test]`s** (`tests/sigint_abort_integration.rs`) —
   run huck on a script via `kill -INT $$` (sets `sigint_flag` directly, so no PTY
   or timing), assert exact stdout + exit code, compared to bash:
   - sequence: `echo a; kill -INT $$; echo b` → stdout `a\n`, rc 130.
   - loop + trailing: `for i in 1 2 3; do echo $i; kill -INT $$; done; echo after`
     → stdout `1\n`, rc 130.
   - function body: `f(){ echo a; kill -INT $$; echo b; }; f; echo after` →
     stdout `a\n`, rc 130 (abort unwinds through the function AND the caller).
   - nested (`if`/`{}`): `if true; then echo a; kill -INT $$; echo b; fi; echo c`
     → stdout `a\n`, rc 130.
   - command substitution: `x=$(echo a; kill -INT $$; echo b); echo "[$x]"; echo after`
     → aborts (rc 130); assert `after` does NOT print. (Match bash; capture bash's
     exact stdout to pin the expected partial output.)
   - **trap handler keeps running:** `trap 'echo c' INT; echo a; kill -INT $$; echo b`
     → stdout `a\nc\nb\n`, rc 0.
   - **trap ignore keeps running:** `trap '' INT; echo a; kill -INT $$; echo b` →
     stdout `a\nb\n`, rc 0.
   - **legit exit code is not an abort:** `bash -c 'exit 130'`-style — a command
     returning 130 WITHOUT a SIGINT does NOT abort a following command:
     `f(){ return 130; }; f; echo still-here` → stdout `still-here\n`, rc 0.
2. **Bash-diff harness** `tests/scripts/sigint_abort_diff_check.sh` (58th) — the
   same vectors as file-args, asserting byte-identical stdout AND identical exit
   code bash↔huck. (No `!`; file-args per L-27.)
3. **PTY test** `tests/sigint_abort_pty.rs` — real Ctrl-C (send `\x03`) during
   `sleep 5` and during a shell-function `while`-loop: huck returns to a fresh
   prompt, a subsequent command runs, `$?`=130, the shell did not exit. Skips
   gracefully if no PTY (mirror the existing `*_pty.rs` harnesses).
4. **Full regression:** entire suite + ALL bash-diff harnesses green — ESPECIALLY
   the job-control / pipeline / completion PTY suites and the existing trap tests.
   `clippy` clean.

## Edge cases & notes
- **Flag clearing discipline:** `check_interrupt` consumes (clears) `sigint_flag`
  on observation, so a single SIGINT aborts exactly once and the next top-level
  prompt/command starts clean. The interactive REPL relies on rustyline for a
  Ctrl-C at the prompt itself (unchanged, `ReadResult::Interrupted`).
- **`set -e` interaction:** an `Interrupted` propagates regardless of `errexit`
  suppression depth — it is control flow, not a failing-command status.
- **`$PIPESTATUS`/pipeline:** if a pipeline is interrupted, it propagates
  `Interrupted` after the wait; `$?`=130. (The per-stage statuses are already
  computed; the pipeline result becomes `Interrupted`.)
- **Subshell `( … )`:** a forked subshell that catches SIGINT aborts its own body;
  the parent observes the child's SIGINT death (trigger (b)) or its own flag and
  unwinds — same as any forked stage.
- **Git safety:** implementer subagents must NOT `git checkout <sha>` (a detached
  HEAD lost commits in a prior iteration); the controller verifies the branch tip
  before merging. Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
