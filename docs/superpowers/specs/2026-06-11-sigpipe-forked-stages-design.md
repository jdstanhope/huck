# huck v137 — SIGPIPE default disposition in forked stages Design

**Status:** approved design, ready for implementation plan.
**Implements:** stop the `huck: printf: Broken pipe (os error 32)` spam (and the
runaway producer) when a builtin/function pipeline stage writes to a pipe whose
reader has closed. Reset `SIGPIPE` to `SIG_DFL` in forked in-process stages /
subshells so the producer dies silently with status 141 like bash; keep the main
shell's `SIGPIPE` ignored but make builtins report a broken pipe silently (rc 141)
instead of printing an error. This is a Tier-1 bug.
**Branch (impl):** `v137-sigpipe-forked-stages`.

## Background — measured root cause

The Rust runtime sets `SIGPIPE` to `SIG_IGN` at process startup. So by default
huck (a Rust program) never dies on a broken pipe — `write(2)` returns `EPIPE`.

- **External pipeline stages are unaffected.** `std::process::Command` resets
  `SIGPIPE` to `SIG_DFL` in the child before `exec`. Verified: `huck -c 'yes | head -3'`
  terminates cleanly (`yes` dies on SIGPIPE), identical to bash.
- **Forked in-process stages and subshells ARE affected.** Builtins, functions,
  compounds, and `( … )` subshells run as a pipeline stage via `libc::fork()`
  WITHOUT exec, through `fork_and_run_in_subshell` (`src/executor.rs`). That child
  resets `SIGTSTP`/`SIGTTIN`/`SIGTTOU` to `SIG_DFL` (executor.rs:~4729) but leaves
  `SIGPIPE` at the Rust-runtime `SIG_IGN`. A forked builtin producer (`printf`,
  `echo`, a function loop) writing to a closed pipe therefore gets `EPIPE` back
  from `write(2)`, the builtin prints `huck: <name>: Broken pipe (os error 32)`
  (`builtins.rs:358`/`:364`/`:2844`), and the enclosing loop keeps running to
  completion instead of the stage being killed.

Measured (release binary), bash vs huck:

```
$ bash -c '{ for i in $(seq 1 100000); do echo $i; done; } | { read x; }; echo ${PIPESTATUS[*]}'
141 0                       # producer killed by SIGPIPE, silent

$ huck -c '{ for i in $(seq 1 100000); do echo $i; done; } | { read x; }; echo ${PIPESTATUS[*]}'
huck: echo: Broken pipe (os error 32)   # ×thousands
huck: echo: Broken pipe (os error 32)
1 0                         # producer ran to completion, exited 1
```

The same mechanism explains the reported `nvm ls` symptom: Ctrl-C kills the
(external) consumer stage; the huck builtin/function producer then keeps writing
into the dead pipe → EPIPE spam → runs to completion. It is NOT a SIGINT-delivery
bug; it is the missing `SIGPIPE` default disposition. (External commands in the
pipeline still receive SIGINT and die correctly.)

## Architecture — two changes

### Change 1 — reset `SIGPIPE` to `SIG_DFL` in the forked-stage child (core fix)

In the `fork_and_run_in_subshell` child block (`src/executor.rs:~4727`), alongside
the existing job-control signal resets:

```rust
libc::signal(libc::SIGTSTP, libc::SIG_DFL);
libc::signal(libc::SIGTTIN, libc::SIG_DFL);
libc::signal(libc::SIGTTOU, libc::SIG_DFL);
// v137: a forked in-process stage / subshell must die on a broken pipe like
// bash (Rust's runtime leaves SIGPIPE at SIG_IGN). Skip when the shell has a
// PIPE trap installed so the user's disposition is respected.
if !pipe_trap_installed {
    libc::signal(libc::SIGPIPE, libc::SIG_DFL);
}
```

`pipe_trap_installed` is computed in the PARENT before the fork (so the child does
no `HashMap` work post-fork) as:

```rust
let pipe_trap_installed = shell.trap_sigids.contains_key(&libc::SIGPIPE);
```

`trap_sigids` holds a `SIGPIPE` key for BOTH the ignore form (`trap '' PIPE`) and
the handler form (`trap 'cmd' PIPE`) — see `traps::install`. When a PIPE trap is
set, signal-hook has installed a real OS handler that the fork inherits, so
leaving `SIGPIPE` untouched correctly preserves it. **Note:** `trap … PIPE` is
currently REJECTED by huck (`signal 13: cannot reset ignored signal`, because
Rust's startup `SIG_IGN` puts SIGPIPE in `ignored_at_startup_set()`), so
`pipe_trap_installed` is always `false` today and the guard branch is presently
unreachable. It is kept for correctness should PIPE traps become settable later
(a separate, out-of-scope item — the existing M-22 limitation). The guard adds no
behavioral change today: forked stages always reset to `SIG_DFL`.

**Effect.** A forked builtin/function/compound/subshell producer writing to a
closed pipe is killed by `SIGPIPE` the instant the consumer closes the read end:
no `write_all` error path runs, no message prints, the stage exits with signal 13,
and `wait_pipeline_raw` maps that to exit status `128 + 13 = 141` (the existing
`status.signal().map(|s| 128 + s)` path, executor.rs:~2730) — byte-identical to
bash for the producer stage status and silent stderr.

**Scope of Change 1.** `fork_and_run_in_subshell` is the single fork-without-exec
site for InProcess pipeline stages AND `( … )` subshells, so this one edit covers
both. A subshell child is a child process; `SIG_DFL` SIGPIPE there matches bash.

**Explicitly NOT touched:**
- The external child pre-exec (`reset_job_control_signals_in_child`,
  executor.rs:~4680): `std::process::Command` already resets SIGPIPE to `SIG_DFL`
  before exec, so external stages are already correct (verified). Leave as-is.
- `spawn_heredoc_writer` (executor.rs:~2602): a forked writer PROCESS that already
  handles `EPIPE` manually (breaks out of its write loop) and must close its write
  end cleanly rather than die mid-body. It keeps `SIG_IGN` + manual EPIPE handling
  — unchanged.

### Change 2 — suppress a broken pipe in main-process builtins (keep the shell alive)

The MAIN shell process keeps `SIGPIPE = SIG_IGN` (unchanged) so a broken pipe can
NEVER kill an interactive shell. A builtin running unforked in the main process
(a single, non-pipeline command — e.g. `printf x >namedpipe` whose reader closed)
will still get `EPIPE` from `write_all`. To match bash's OUTPUT there, route the
builtin stdout-write error sites through one shared helper:

```rust
// In builtins.rs. Returns the exit code; prints nothing for a broken pipe.
fn report_write_error(name: &str, e: &std::io::Error) -> i32 {
    if e.kind() == std::io::ErrorKind::BrokenPipe {
        141 // 128 + SIGPIPE; silent, matches bash's signal-killed status
    } else {
        eprintln!("huck: {name}: {e}");
        1
    }
}
```

Apply at the builtin stdout-write error sites that currently do
`eprintln!("huck: <name>: {e}"); return …` for an `out.write_all(...)` failure:
- `echo` — `builtins.rs:357-358` (body) and `:362-364` (trailing newline). Both
  collapse to one `report_write_error("echo", &e)` returning its code.
- `printf` — `builtins.rs:2843-2845`. Use `report_write_error("printf", &e)`.

These are the only builtins that emit looped/bulk stdout and are the observed
offenders. Other builtins that write stdout and report an error (e.g. `jobs`,
`pwd`, `dirs`, `read`) MAY adopt the helper for consistency, but that is optional
polish — the implementer should apply it to `echo` and `printf` (required) and to
any other stdout-write site only if it is a trivial, in-place swap with no
behavior change for the non-broken-pipe path.

**Why 141 and silence.** bash, when its (forked) builtin stage is killed by
SIGPIPE, reports status 141 and prints nothing. In the main-process case bash
would actually be signal-killed (so the whole `bash -c …` exits 141); huck instead
keeps running and returns 141 from the builtin. For `huck -c '<builtin>' | head`
the observable result (truncated stdout, exit 141, empty stderr) is identical to
bash. For an interactive shell, surviving is strictly better than dying.

## Behaviour matrix (target = bash)

| Case | bash | huck after v137 |
|---|---|---|
| `printf-loop \| head -3` (builtin producer, forked stage) | producer SIGPIPE-killed, silent, stage rc 141 | identical |
| `echo-loop \| read x` (builtin producer) | silent, stage rc 141 | identical |
| `func-loop \| head` (function producer, forked stage) | silent, stage rc 141 | identical |
| `( printf-loop ) \| head` (subshell producer) | silent, stage rc 141 | identical |
| `yes \| head` (external producer) | SIGPIPE-killed, silent | already identical (unchanged) |
| `nvm ls`, Ctrl-C the consumer | producer dies, returns to prompt clean | producer dies on SIGPIPE, no spam |
| `huck -c '<builtin>' \| head` (main-proc builtin) | shell SIGPIPE-killed, rc 141, silent | builtin returns 141, silent, shell survives |
| `trap '' PIPE; …` | trap respected | `trap … PIPE` still rejected (unchanged limitation); guard ready |

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | In `fork_and_run_in_subshell`: compute `pipe_trap_installed = shell.trap_sigids.contains_key(&libc::SIGPIPE)` pre-fork; in the child, after the three job-control resets, `if !pipe_trap_installed { libc::signal(libc::SIGPIPE, libc::SIG_DFL); }`. |
| `src/builtins.rs` | Add `report_write_error(name, &io::Error) -> i32` helper; route the `echo` (357/364) and `printf` (2844) stdout-write error sites through it. |
| `tests/scripts/sigpipe_diff_check.sh` (NEW) | Bash-diff harness (the 57th): producer-into-`head`/early-`read`, `printf`/`echo`/function producers — byte-identical stdout AND empty stderr vs bash. |
| `tests/sigpipe_integration.rs` (NEW) | Producer-stage status == 141; overall pipeline rc matches bash; bounded-output guard (producer does NOT run to completion); main-process builtin EPIPE → rc 141, no "Broken pipe" on stderr. |
| `docs/bash-divergences.md` | Reopen Tier-1 0→1 during work, DELETE on merge (this IS the fix). Add the two low-impact residual notes (trap-PIPE-unsettable already exists under M-22; main-proc-EPIPE-returns-141 as a brief `[intentional]`/`[low]` entry). |

## Testing

1. **Bash-diff harness** `tests/scripts/sigpipe_diff_check.sh` (gold standard) —
   each fragment run through bash and huck, assert byte-identical stdout AND
   identical (empty) stderr:
   - `{ for i in $(seq 1 5000); do printf '%d\n' "$i"; done; } | head -3`
   - `{ for i in $(seq 1 5000); do echo "$i"; done; } | head -3`
   - `seq 1 5000 | { read x; echo "first=$x"; }` (external producer control)
   - `f(){ local i=0; while [ "$i" -lt 5000 ]; do echo "$i"; i=$((i+1)); done; }; f | head -2`
   - `( for i in $(seq 1 5000); do echo "$i"; done ) | head -2` (subshell producer)
   NOTE on the harness runner: per L-27, history expansion runs on piped
   non-interactive stdin, so run fragments as FILE-ARGS (write each to a temp
   script and `huck script`), matching the existing harnesses' convention — NOT
   piped via stdin (the `seq`/`!`-free fragments are safe either way, but follow
   the file-arg convention for consistency).
2. **Integration `#[test]`s** (`tests/sigpipe_integration.rs`), run via the huck
   binary, asserting exact behavior (compare to bash where noted):
   - **producer stage status 141**:
     `{ for i in $(seq 1 5000); do echo $i; done; } | { read x; }; echo ${PIPESTATUS[*]}`
     → stdout `141 0` (matches bash), stderr empty.
   - **overall pipeline rc**: `printf-loop | head -1` → rc 0, stdout 1 line, stderr empty.
   - **bounded output (no runaway)**: a producer that would print 5000 lines into a
     `head -1` emits exactly 1 line of stdout and ZERO "Broken pipe" lines on
     stderr (the assertion that the v137 fix actually fires — pre-fix this spews
     thousands of stderr lines).
   - **main-process builtin EPIPE**: a single (non-pipeline) builtin whose stdout
     is a closed pipe returns 141 with no "Broken pipe" message and the shell stays
     alive (subsequent command still runs). Construct via a redirect to a reader
     that closes, or document if not portably constructible and rely on the helper
     unit test instead.
   - **`report_write_error` unit test** (in `builtins.rs` tests): `BrokenPipe` → 141
     no print; a non-EPIPE error → 1 (the print is a side effect, assert the code).
3. **Full regression:** entire suite + ALL bash-diff harnesses green — ESPECIALLY
   the job-control / pipeline PTY suites (`pty_interactive`, `subshell_pipeline_pty`,
   `completion_jobcontrol_pty`, `subshell_tty_pty`) which exercise the forked-stage
   and subshell paths this change touches. Ctrl-Z stop, subshell tty hand-off, and
   completion job-control must be unaffected. `clippy` clean.

## Edge cases & notes

- **Status mapping already exists.** `wait_pipeline_raw` already maps a
  signal-terminated child to `128 + signum` (executor.rs:~2730), so a SIGPIPE-killed
  forked stage naturally reports 141 — no status-mapping change needed.
- **`set -o pipefail` / `$PIPESTATUS`.** Unchanged: the producer now contributes 141
  to `$PIPESTATUS` (matching bash). Under `pipefail` the pipeline's rc becomes the
  rightmost non-zero, i.e. 141 if the consumer succeeded — same as bash.
- **Subshell standalone (`( cmd )` not in a pipeline).** The reset still applies (a
  subshell is a child process); for a standalone subshell whose stdout is the
  terminal, SIGPIPE never fires, so no behavior change. Correct and bash-like.
- **Heredoc writer untouched.** It must close cleanly after delivering the body;
  it keeps `SIG_IGN` + manual EPIPE handling. Do not route it through the reset.
- **Do not reset `SIGPIPE` process-wide / in the main shell.** That would let a
  broken pipe terminate the interactive shell — explicitly rejected.
- **Git safety.** Implementer subagents must NOT `git checkout <sha>` (detached
  HEAD lost commits in a prior iteration); the controller verifies the branch tip
  before merging.
