# huck v137 — SIGPIPE default disposition Design

**Status:** approved design (revised after prototype), ready for implementation plan.
**Implements:** stop the `huck: printf: Broken pipe (os error 32)` spam and the
runaway producer when a shell command writes to a pipe whose reader has closed.
Restore the OS-default `SIGPIPE` disposition (`SIG_DFL`) process-wide — exactly
what bash does — so a producer is killed silently with status 141 instead of
looping on `EPIPE`. This is a Tier-1 bug.
**Branch (impl):** `v137-sigpipe-forked-stages`.

## Background — measured root cause

The Rust runtime sets `SIGPIPE` to `SIG_IGN` at process startup. So by default
huck (a Rust program) never dies on a broken pipe — `write(2)` returns `EPIPE`
instead of the writer being killed.

- **External pipeline stages already behave correctly.** `std::process::Command`
  resets `SIGPIPE` to `SIG_DFL` in the child before `exec`. Verified:
  `huck -c 'yes | head -3'` terminates cleanly, identical to bash.
- **Everything that writes from huck's own (forked or main) process is wrong.**
  A builtin/function/compound/subshell producer writing to a closed pipe gets
  `EPIPE`, the builtin prints `huck: <name>: Broken pipe (os error 32)`
  (`builtins.rs:358`/`:364`/`:2844`), and the enclosing loop keeps running.

Measured (release binary), bash vs huck — both a forked-stage producer and a
main-process producer are broken:

```
# forked builtin producer (pipeline stage)
$ bash -c '{ for i in $(seq 1 100000); do echo $i; done; } | { read x; }; echo ${PIPESTATUS[*]}'
141 0                       # producer SIGPIPE-killed, silent
$ huck -c '...same...'
huck: echo: Broken pipe (os error 32)   # ×thousands
1 0                         # producer ran to completion

# main-process producer (huck's own stdout is an external closed pipe)
$ bash -c 'while true; do printf "x\n"; done' | head -c1   ->  rc 141, terminates instantly
$ huck -c 'while true; do printf "x\n"; done' | head -c1    ->  never terminates (timeout-killed), 100k+ stderr lines
```

The reported `nvm ls` symptom is the forked-producer case: Ctrl-C kills the
(external) consumer; the huck builtin/function producer keeps writing into the
dead pipe → EPIPE spam → runs to completion. It is NOT a SIGINT-delivery bug;
external stages still receive SIGINT and die correctly.

**bash's model (the fix target).** bash runs with `SIGPIPE` at `SIG_DFL`
*everywhere* — `trap -p SIGPIPE` is empty (default disposition). Its interactive
shell does not die on a broken pipe simply because an interactive shell's stdout
is the *terminal*, never a pipe; and pipeline stages are forked children. The only
time bash's shell process itself dies on SIGPIPE is exactly the
`bash -c '…' | head` case — where dying (rc 141) is correct.

## Architecture — restore SIG_DFL, process-wide

huck must stop overriding the OS default. The fix is to set `SIGPIPE` back to
`SIG_DFL` once at startup; forked children then inherit it, and the main process
dies on a broken stdout pipe exactly like bash.

### Change 1 — reset `SIGPIPE` to `SIG_DFL` at shell startup (the fix)

In `src/shell.rs`, `install_job_control_signals()` (called once from `run()` at
shell.rs:233, before any command in EVERY mode — `-c`, file, interactive),
restore the default after the existing job-control `SIG_IGN`s:

```rust
fn install_job_control_signals() {
    for sig in [libc::SIGTSTP, libc::SIGTTIN, libc::SIGTTOU] {
        let prev = unsafe { libc::signal(sig, libc::SIG_IGN) };
        if prev == libc::SIG_ERR {
            eprintln!("huck: warning: could not ignore signal {sig}");
        }
    }
    // Rust's runtime sets SIGPIPE to SIG_IGN at startup; restore the OS default
    // so huck (and the stages it forks) die on a broken pipe like bash, instead
    // of getting EPIPE back from write(2) and looping. bash runs with SIGPIPE at
    // SIG_DFL everywhere; an interactive shell survives because its stdout is the
    // terminal, never a pipe.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }
}
```

**Prototype-verified** (one line, built release): forked producer →
`stages=141 0`, no spam; `huck -c 'while true; do printf x; done' | head -c1` →
rc 141, terminates, ZERO stderr lines; both byte-identical to bash.

### Change 2 — explicit `SIGPIPE`→`SIG_DFL` in the forked-stage child (belt-and-suspenders + subshell semantics)

In the `fork_and_run_in_subshell` child (`src/executor.rs:~4729`), alongside the
existing `SIGTSTP`/`SIGTTIN`/`SIGTTOU` resets, add:

```rust
libc::signal(libc::SIGTSTP, libc::SIG_DFL);
libc::signal(libc::SIGTTIN, libc::SIG_DFL);
libc::signal(libc::SIGTTOU, libc::SIG_DFL);
libc::signal(libc::SIGPIPE, libc::SIG_DFL); // v137
```

With Change 1 the child already inherits `SIG_DFL`, so this is redundant in the
common (no-PIPE-trap) case. It is kept because it is also CORRECT for the
PIPE-trap case: bash resets a trapped signal to default inside a subshell
(forked stage). A top-level `trap '…' PIPE` installs a signal-hook handler in the
MAIN process (which fires there, matching bash); a forked stage must NOT inherit
that handler — it must die on SIGPIPE like a bash subshell. The unconditional
reset gives exactly that. (huck cannot distinguish the `trap '' PIPE` ignore-form
from a handler at the OS level — both are signal-hook handlers per the M-22
note — so the "ignore-stays-ignored-in-subshell" nuance remains the existing
M-22 limitation; immaterial here.)

### Change 3 — preserve the heredoc writer's manual EPIPE handling

`spawn_heredoc_writer` (`src/executor.rs:~2608`) forks a writer PROCESS from the
main process that writes a heredoc body to a pipe and already handles `EPIPE`
manually (breaks its write loop, `_exit`s cleanly — v134). With Change 1 it would
inherit `SIG_DFL` and be killed by SIGPIPE on an early-closing consumer instead.
That is functionally equivalent (its status is reaped and discarded, never a job
or `$!`), but to keep v134's well-tested behavior byte-for-byte unchanged, restore
`SIG_IGN` in the writer child before its write loop:

```rust
if pid == 0 {
    unsafe { libc::close(r); libc::signal(libc::SIGPIPE, libc::SIG_IGN); } // v137: keep manual EPIPE handling
    // ... existing write loop with the errno==EPIPE break ...
}
```

### Not changed
- **`builtins.rs` is untouched.** With `SIGPIPE = SIG_DFL`, the `echo`/`printf`
  `out.write_all` EPIPE branch is unreachable in normal use (the process is killed
  before `write_all` returns). The existing `eprintln!("huck: <name>: {e}")` arms
  remain only for genuine non-EPIPE write failures (e.g. `ENOSPC`) and for the
  rare `trap '' PIPE`-ignore context — which is roughly what bash reports there
  too. No suppress helper is needed (the SIG_IGN-based design considered earlier
  is dropped — process-wide SIG_DFL makes it unnecessary).
- **External child pre-exec** (`reset_job_control_signals_in_child`): unchanged
  (`std::process::Command` already resets SIGPIPE).

## Behaviour matrix (target = bash)

| Case | bash | huck after v137 |
|---|---|---|
| `printf-loop \| head -3` (forked builtin stage) | producer SIGPIPE-killed, silent, stage rc 141 | identical (verified) |
| `echo-loop \| read x` (forked builtin stage) | silent, stage rc 141 | identical (verified) |
| `func-loop \| head` (forked function stage) | silent, stage rc 141 | identical |
| `( printf-loop ) \| head` (subshell producer) | silent, stage rc 141 | identical |
| `yes \| head` (external producer) | SIGPIPE-killed, silent | already identical (unchanged) |
| `huck -c '<loop>' \| head` (main-proc producer) | shell SIGPIPE-killed, rc 141, silent | identical: rc 141, terminates, silent (verified) |
| `nvm ls`, Ctrl-C the consumer | producer dies, clean prompt | producer dies on SIGPIPE, no spam |
| `trap '…' PIPE; echo set` | accepted | accepted (now settable — was rejected) |

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell.rs` | `install_job_control_signals()`: add `libc::signal(libc::SIGPIPE, libc::SIG_DFL)` after the job-control `SIG_IGN`s. |
| `src/executor.rs` | (a) `fork_and_run_in_subshell` child: add `libc::signal(libc::SIGPIPE, libc::SIG_DFL)` beside the job-control resets. (b) `spawn_heredoc_writer` child: add `libc::signal(libc::SIGPIPE, libc::SIG_IGN)` to preserve its manual EPIPE handling. |
| `tests/scripts/sigpipe_diff_check.sh` (NEW, 57th) | Bash-diff harness: forked `printf`/`echo`/function/subshell producers into `head`/early-`read`, plus the `trap '' PIPE`-accepted case — stdout AND stderr byte-identical to bash. |
| `tests/sigpipe_integration.rs` (NEW) | Producer-stage status 141; pipeline rc; bounded-output guard (no runaway / no "Broken pipe" on stderr); main-process `huck -c '<loop>'`-style producer terminates rc 141 silent; `trap … PIPE` no longer errors. |
| `docs/bash-divergences.md` | Reopen Tier-1 0→1 during work, DELETE on merge (this IS the fix). No new deferred entry needed; optionally note under M-22 that `trap … PIPE` is now settable (the prior "cannot reset ignored signal" rejection is gone). |

## Testing

Because a SIGPIPE-killed producer can interleave with consumer output under a
single `2>&1` capture, the harness captures **stdout and stderr separately** (a
deliberate deviation from the combined-capture convention of other harnesses) and
asserts: huck stdout == bash stdout, AND huck stderr == bash stderr (both empty).

1. **Bash-diff harness** `tests/scripts/sigpipe_diff_check.sh` (gold standard) —
   for each fragment, run through bash and huck via `-c`, capture stdout and
   stderr to separate files, assert both match:
   - `{ for i in $(seq 1 5000); do printf '%d\n' "$i"; done; } | head -3`
   - `{ for i in $(seq 1 5000); do echo "$i"; done; } | head -3`
   - `f(){ local i=0; while [ "$i" -lt 5000 ]; do echo "$i"; i=$((i+1)); done; }; f | head -2`
   - `( for i in $(seq 1 5000); do echo "$i"; done ) | head -2`
   - `seq 1 5000 | { read x; echo "first=$x"; }` (external producer control)
   - `trap '' PIPE; echo set-ok` and `trap 'echo h' PIPE; echo set-ok` (now accepted)
   (No `!` in any fragment, so `-c` is safe re: L-27.)
2. **Integration `#[test]`s** (`tests/sigpipe_integration.rs`), via the huck binary
   (`env!("CARGO_BIN_EXE_huck")`, stdin-piped script — no `!`), asserting exact
   behavior:
   - **producer stage status 141:** `{ for i in $(seq 1 5000); do echo $i; done; } | { read x; }; echo ${PIPESTATUS[*]}` → stdout `141 0`, stderr empty.
   - **overall pipeline rc + bounded output:** a 5000-line producer into `head -1`
     emits exactly 1 stdout line and ZERO "Broken pipe" stderr lines (this is the
     assertion that the fix fired — pre-fix it spews thousands).
   - **main-process producer terminates:** run `while true; do printf 'x\n'; done`
     with the child's stdout connected to a reader that closes after 1 byte
     (a `head -c1` wrapper, or a `Stdio` pipe the test drops); assert the huck
     process exits (rc 141) within a short watchdog and prints no "Broken pipe".
     If a closing-reader is not cleanly constructible from the Rust test harness,
     drive it through a small `bash -c 'huck -c "..." | head -c1'` shell-out and
     assert termination + empty huck stderr.
   - **`trap … PIPE` settable:** `trap 'echo h' PIPE; echo ok` → stdout `ok`, rc 0,
     no "cannot reset ignored signal" on stderr.
3. **Full regression:** entire suite + ALL bash-diff harnesses green — ESPECIALLY
   the job-control / pipeline PTY suites (`pty_interactive`, `subshell_pipeline_pty`,
   `completion_jobcontrol_pty`, `subshell_tty_pty`) and the heredoc tests (Change 3
   touches the writer). Ctrl-Z stop, subshell tty hand-off, completion job-control,
   and large-heredoc delivery must be unaffected. `clippy` clean.

## Edge cases & notes

- **Status mapping already exists.** `wait_pipeline_raw` maps a signal-terminated
  child to `128 + signum` (executor.rs:~2730), so a SIGPIPE-killed stage reports
  141 with no new code.
- **`set -o pipefail` / `$PIPESTATUS`.** The producer now contributes 141 to
  `$PIPESTATUS` (matching bash); under `pipefail` the pipeline rc becomes the
  rightmost non-zero — same as bash.
- **Interactive safety (verified by reasoning + prototype).** An interactive
  shell's stdout is the terminal, so the main process never writes to a broken
  pipe and never dies; pipeline stages are forked; `$(…)` capture writes to an
  in-memory buffer, and command-sub children write while huck *reads*. The only
  scenario where the huck process itself dies on SIGPIPE is `huck …| head`, where
  bash dies too. No interactive path writes to a broken pipe.
- **`trap … PIPE` now settable.** Restoring `SIG_DFL` at startup (before the lazy
  `ignored_at_startup_set()` snapshot is taken on the first trap call) removes
  SIGPIPE from the ignored-at-startup set, so `trap … PIPE` no longer errors with
  "cannot reset ignored signal". A top-level PIPE trap fires via huck's existing
  flag-based dispatch; the forked-stage reset (Change 2) means a trap does not
  fire inside a pipeline subshell, matching bash. Full PIPE-trap-firing semantics
  ride on the existing trap machinery; only the "settable" smoke is asserted here.
- **Heredoc writer untouched in spirit.** Change 3 keeps its manual EPIPE handling
  so the v134 large-heredoc behavior is byte-for-byte preserved.
- **Git safety.** Implementer subagents must NOT `git checkout <sha>` (detached
  HEAD lost commits in a prior iteration); the controller verifies the branch tip
  before merging.
