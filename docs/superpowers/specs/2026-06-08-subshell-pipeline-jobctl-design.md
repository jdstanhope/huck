# huck v108 — pipeline inside a subshell hangs on a tty (M-104) Design

**Status:** approved design, ready for implementation plan.
**Implements:** fixing a deadlock where a **multi-stage pipeline inside a subshell**
(`( cmd1 | cmd2 )`) hangs whenever huck has a **controlling terminal** (interactive
REPL, or even `huck -c` run under a tty). New **M-104** `[fixed v108]` (Tier-1,
high — it hangs the shell).
**Why now:** sourcing `~/.bashrc` hangs. Root-caused this session: nvm's
`nvm_resolve_alias` runs, at load time, `$( ( nvm_alias … | head -n1 | tail -n1 )
|| nvm_echo )` — a pipeline inside a subshell — which deadlocks, so
`source ~/.bashrc` never returns. (Everything else seen along the way — mise's
`declare -p … 2>/dev/null` leak via M-90, `export -a`/M-89, the `${arr[@]:-…}`
array-modifier limit — is downstream/harmless.)
**Branch (impl):** `v108-subshell-pipeline-jobctl`.

## Minimal reproduction (the contract)

In a pty (real terminal):
```
( echo hi | cat )            # HANG  — prints "hi", then never returns
( echo hi | head -n 1 )      # HANG
( echo hi )                  # OK    — no pipeline
echo hi | cat                # OK    — pipeline NOT in a subshell
echo hi | ( cat )            # OK    — subshell is a single-command stage
```
Same `( echo hi | cat )`:
- **bash** (interactive or `-c`): works.
- **huck script mode** / stdin not a tty (`</dev/null`): works.
- **huck with a tty** (REPL or `huck -c '( echo hi | cat )'` under a pty): **HANG**.

So the trigger is precisely: a forked **subshell** running a **multi-stage
pipeline**, with a **controlling terminal** present. The pipeline *produces its
output* ("hi" is printed) and then the subshell's **wait deadlocks**.

## Root cause (verified)

Two things combine inside `fork_and_run_in_subshell` (`src/executor.rs`, child
branch ~`:4108`):

1. The subshell child **resets job-control signals to default**:
   ```rust
   libc::signal(libc::SIGTSTP, libc::SIG_DFL);
   libc::signal(libc::SIGTTIN, libc::SIG_DFL);
   libc::signal(libc::SIGTTOU, libc::SIG_DFL);
   libc::setpgid(0, pgid_target);
   ```
2. The subshell then runs the inner pipeline via `run_multi_stage`
   (`src/executor.rs:2983`), whose job-control behavior is gated on
   `let interactive = matches!(sink, StdoutSink::Terminal);` (`:2990`). With a
   Terminal sink the stages are **`setpgid`'d into a new process group**
   (`pgid_target = if interactive { first_pid.unwrap_or(0) } else { 0 }`, `:3026`).

Result: inside the forked subshell (which is **not** the terminal's foreground
process group), the inner pipeline stages are placed in a background process group
with default job-control signal dispositions; with a controlling terminal this
deadlocks the subshell's wait for the pipeline (a stopped/SIGTTOU/SIGTTIN
interaction). Without a controlling terminal (script mode / non-tty) the same code
can't be stopped by the terminal, so it completes — which is why every
non-interactive test passes and only the real interactive session hangs.

bash never does this: a subshell is a forked child that runs its commands **without
starting new job-control process groups** — the inner pipeline's children stay in
the subshell's own process group, and the subshell keeps `SIGTTOU` effectively
ignored (as the interactive shell does), so nothing stops.

## The fix

**A forked subshell must not perform interactive job-control process-grouping for
its inner pipelines.** Add a "we are inside a forked subshell" flag to `Shell` and
gate the `run_multi_stage` job-control path on it, so a subshell's inner pipeline
runs on the **non-job-control path** (the same path that already works in script
mode), keeping the stages in the subshell's process group.

### Section 1 — `Shell.in_subshell` flag (`src/shell_state.rs`)
Add `pub in_subshell: bool` (default `false`). It marks that the current process is
a forked subshell child (set after `fork()` in the subshell child, before it runs
any inner command).

### Section 2 — Set it in the subshell child (`src/executor.rs`)
In `fork_and_run_in_subshell`'s child branch (~`:4108`, right after the
`setpgid`/signal-reset block, before it dispatches into `run_command`/`execute`),
set `shell.in_subshell = true;`. (The child is a fresh forked process with its own
copy of `shell`; the parent's `shell.in_subshell` is unaffected.) Background
subshells (`run_background_subshell`) and the capture path go through the same
fork helper, so they inherit the flag consistently.

### Section 3 — Gate the job-control grouping (`src/executor.rs:2990`)
Change:
```rust
let interactive = matches!(sink, StdoutSink::Terminal);
```
to:
```rust
// Job-control process-grouping (setpgid into a foreground-style group) is only
// correct in the top-level interactive shell. Inside a forked subshell it places
// the inner pipeline in a background group with default SIGTTOU/SIGTTIN handling,
// which deadlocks the subshell's wait when a controlling terminal is present
// (M-104). A subshell's inner pipeline uses the non-job-control path, matching
// bash (stages stay in the subshell's process group).
let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_subshell;
```
This is the minimal, targeted change. With `in_subshell` true, `pgid_target` is
`0` for every stage (the non-tty path), which already works.

### Section 4 — Verify the exact mechanism against the repro
Job control is subtle; the implementer MUST confirm with the pty repro that
Section 3 alone resolves `( echo hi | cat )` on a tty. If a residual hang remains
(e.g. the subshell child's `SIGTTOU`→`SIG_DFL` reset still stops a stage on a tty
write), the secondary fix is to **not reset `SIGTTOU`/`SIGTTIN` to `SIG_DFL` for a
subshell that continues running shell code** (keep the parent's dispositions; the
re-forked/exec'd pipeline stages reset their own signals at exec). Apply the
smallest change that makes the repro pass without regressing the working cases
below.

## Must-not-regress
- Top-level interactive pipeline `echo hi | cat` (job control intact — still works).
- `( echo hi )` (single-command subshell), `echo hi | ( cat )` (subshell as a stage).
- Script-mode / non-tty pipelines and subshells (byte-unchanged — `in_subshell`
  only changes the Terminal-sink job-control grouping).
- Background pipelines / `&`, Ctrl-C/Ctrl-Z handling at the top level, `$PIPESTATUS`.
- Capture-mode command substitution `$( a | b )` (already works; sink=Capture →
  `interactive` already false, so unaffected).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | add `pub in_subshell: bool` (default false) |
| `src/executor.rs` | set `shell.in_subshell = true` in the subshell child; gate `interactive` on `!shell.in_subshell` (`:2990`); apply the Section-4 signal fix only if needed |
| `tests/subshell_pipeline_pty.rs` (or extend `tests/pty_interactive.rs`) | NEW — pty regression: `( echo hi \| cat )` under a tty completes |
| `tests/*_integration.rs` | non-pty: `( echo hi \| cat )` output/exit unchanged |
| `docs/bash-divergences.md`, `README.md` | M-104 `[fixed v108]`; changelog; README row |

## Testing

1. **pty regression (the bug)**: spawn `huck` (or `huck -c`) under a pty, run
   `( echo hi | cat ); echo DONE`, assert `hi` AND `DONE` appear within a timeout
   (today: `hi` then hang). Also `( echo hi | head -n 1 | tail -n 1 )`. Use the
   existing pty harness pattern (`tests/pty_interactive.rs`); a generous wall-clock
   bound (e.g. 5 s) detects the regression.
2. **Non-pty equivalence (vs bash)**: `( echo hi | cat )`, `( printf 'a\nb\n' | tail -n 1 )`,
   `x=$( ( echo hi | cat ) ); echo "[$x]"` → byte-identical to bash; `( echo hi )`,
   `echo hi | ( cat )` unchanged.
3. **Job-control not broken**: a top-level interactive pipeline still runs and can
   be Ctrl-C'd (pty); `$PIPESTATUS` of `false | true` is correct inside and outside
   a subshell.
4. **Payoff**: `source /usr/lib/.../nvm.sh` then `nvm_resolve_alias default`
   completes (pty); ideally a full `huck` that sources the user's prompt stack no
   longer hangs (manual / user-confirmed, since it needs their `~/.bashrc`).
5. **Regression**: full suite (2719+), all 32 harnesses, the existing
   `pty_interactive` suite.

## Edge cases & notes
- **Nested subshells** `( ( a | b ) )`: the inner subshell child also forks via the
  same helper → `in_subshell` stays true → inner pipeline still non-job-control.
- **`huck -c '( a | b )'` under a tty**: `is_interactive` is false but the bug still
  reproduces (sink=Terminal), so the fix correctly keys off `in_subshell`/sink, not
  `is_interactive`.
- This does not change job control for top-level jobs; only subshell-internal
  pipelines lose the (incorrect) separate process group.
