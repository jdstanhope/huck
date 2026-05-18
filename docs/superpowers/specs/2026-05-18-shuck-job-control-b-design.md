# shuck Job Control Design (Sub-project B: fg, bg, Ctrl-Z)

**Status:** Approved 2026-05-18.
**Builds on:** [sub-project A](2026-05-16-shuck-job-control-design.md) (trailing `&`, `jobs`, `wait`, SIGCHLD reaping).

## Overview

Sub-project A added background pipelines and the prompt-time reaper. This
iteration finishes the interactive half of job control: pausing a
foreground job with Ctrl-Z, resuming it in the foreground with `fg`, and
resuming a stopped job in the background with `bg`. With this in place,
interactive programs that expect to own the terminal (vim, less, top)
work, and the standard Ctrl-Z → `bg` → `fg` workflow is available.

## Goals

- Ctrl-Z pauses the current foreground pipeline and returns to the prompt.
- `fg` resumes the current job in the foreground.
- `bg` resumes the current stopped job in the background.
- The `jobs` table shows `Stopped` (and `Stopped (tty input/output)`) state.
- `wait` blocks until no jobs are Running *or* Stopped (bash semantics).
- Foreground programs that want the controlling terminal (vim, less) work.
- Shuck never suspends itself when handling Ctrl-Z at an empty prompt.

## Non-goals (deferred to sub-project C)

- Job specifiers: `%1`, `%+`, `%-`, `%cmd` for `fg`/`bg`/`wait`/`kill`.
- `kill` builtin (the external `kill` still works).
- `disown` builtin.
- Backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`).
- `set -m` / `set +m` toggle (shuck is always interactive).

## Architecture

The single architectural change is that foreground pipelines now spawn
into their own process group and shuck hands the controlling terminal to
that group for the duration of the wait. When the kernel delivers SIGTSTP
to the foreground group (because the user pressed Ctrl-Z), the children
stop, the shell's `waitpid` returns with `WIFSTOPPED`, and shuck takes
the terminal back, registers the job as `Stopped`, and returns to the
prompt. `fg` and `bg` are then thin wrappers around "send SIGCONT and
optionally hand the terminal back to the job."

Shuck itself ignores SIGTSTP/SIGTTIN/SIGTTOU at startup so it can never
suspend itself — important because `tcsetpgrp` from a non-foreground pgrp
would otherwise deliver SIGTTOU to shuck mid-handoff.

## 1. Job state and notifications

`JobState` gains a new variant:

```rust
pub enum JobState {
    Running,
    Stopped(i32),       // NEW: carries the stop signal (SIGTSTP=20, SIGTTIN=21, SIGTTOU=22)
    Done(i32),
    Signaled(i32),
}
```

The SIGCHLD reaper switches from `WNOHANG` to `WNOHANG | WUNTRACED`.
When `libc::WIFSTOPPED(status)` is true, the reaper:

- Looks up the job by pid.
- Sets `state = Stopped(libc::WSTOPSIG(status))`.
- Sets `notified = false` so the next prompt prints the stop line.
- Does **not** remove the job — it is not finished.

`render_state` formats stop states as:

| Signal             | Display                    |
| ------------------ | -------------------------- |
| `SIGTSTP` (20)     | `Stopped`                  |
| `SIGTTIN` (21)     | `Stopped (tty input)`      |
| `SIGTTOU` (22)     | `Stopped (tty output)`     |
| other              | `Stopped (signal N)`       |

The state column width bumps from 20 to 24 to fit `Stopped (tty output)`
without rewrapping. The notification format from v6
(`[N]{flag} {state:<20} {cmd} &`) gets a small refinement: the trailing
`&` is shown only when `state` is `Done` or `Signaled`, and is omitted
for `Stopped`. A `&` next to "Stopped" reads as "still running in the
background," which is wrong. So:

- `[1]+ Stopped              sleep 100`         (no `&`)
- `[1]+ Stopped (tty input)  cat`                (no `&`)
- `[1]+ Done                 echo hi &`          (with `&`)
- `[1]- Exit 1               false &`            (with `&`)

When a stop is detected synchronously by the foreground wait loop, the
notification prints immediately (`\n[N]+ Stopped <cmd>`) because the
shell is about to return to a fresh prompt anyway. When a stop is
detected by the SIGCHLD reaper (e.g., a background job hits the terminal
and gets SIGTTIN), the notification queues for the next prompt via the
same `notified = false` mechanism v6 uses for Done.

## 2. Foreground execution path

The current foreground path runs `waitpid(pid, &mut status, 0)` on each
pipeline stage in shuck's own process group. The new path mirrors the
background path's pgrp setup and adds terminal handoff.

**Spawn (in `run_single` for `Exec` and `run_multi_stage`):**

1. First stage: `process_group(0)` via `CommandExt::process_group`.
2. After spawn, parent calls `unsafe { libc::setpgid(pid, pid) }` (race
   fix, ignore return). Capture `first_pid` as the job's pgid.
3. Subsequent stages: `process_group(first_pid)`.

**Pre-wait:**

```rust
unsafe { libc::tcsetpgrp(0, pgid) };  // give the job the controlling terminal
```

`tcsetpgrp` failures (ENOTTY for non-tty environments, EPERM for races)
are ignored. Job control degrades gracefully — pgrp setup still happens,
just no terminal routing.

**Wait loop:**

```rust
for &pid in &pids {
    let mut status = 0;
    let r = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
    if r < 0 { break; }
    if libc::WIFSTOPPED(status) {
        stopped_sig = Some(libc::WSTOPSIG(status));
        break;
    }
    last_status = status;
}
```

If `stopped_sig` is set, the other pipeline stages are typically stopped
too (Ctrl-Z went to the whole pgrp). We don't drain them here — the
SIGCHLD reaper picks them up at the next prompt and we already have
enough information to register the job.

**Post-wait:**

```rust
unsafe { libc::tcsetpgrp(0, shell_pgid) };  // reclaim the terminal
```

`shell_pgid` is captured once at `Shell::new` via `libc::getpgrp()`.

**On stop:** register a Job with `state = Stopped(sig)`, `pgid`, `pids`,
the original source line as `command`. Print `\n[N]+ Stopped <cmd>` to
stderr. Set `last_status = 128 + sig`. Return.

**On normal exit:** decode the final pid's status as today; no job-table
entry needed. Return.

Builtin-only pipelines (in-process) and single-builtin commands skip all
of this — they don't fork, so there's no pgrp work.

## 3. `fg` and `bg` builtins

Both live in `src/builtins.rs`. No-arg only this iteration; any args
return status 2 with `shuck: <name>: arguments not supported in this version`.

`JobTable` gains:

```rust
pub fn current_id(&self) -> Option<usize>      // the `+` job, Running or Stopped
pub fn current_stopped_id(&self) -> Option<usize>  // the `+` Stopped job
```

**`builtin_fg(args, shell) -> ExecOutcome`:**

1. If `!args.is_empty()`: print error, return `Continue(2)`.
2. `id = shell.jobs.current_id()`; if `None`, print `shuck: fg: no current job`, return `Continue(1)`.
3. Look up the job; capture `pgid`, `pids.clone()`, `command.clone()`.
4. Print `<command>` to stderr (bash echoes the command being foregrounded).
5. Mark job state `Running`, `notified = true` (suppress any stale stop notification).
6. `unsafe { libc::tcsetpgrp(0, pgid) }`.
7. `unsafe { libc::killpg(pgid, libc::SIGCONT) }`.
8. Wait loop identical to the foreground execution path:
   - If `WIFSTOPPED(status)`: take terminal back, transition to `Stopped(sig)`, set `notified=false`, print `\n[N]+ Stopped <cmd>`, return `Continue(128+sig)`.
   - On exit/signal of last pid: take terminal back, transition to `Done(c)`/`Signaled(s)`, remove from job table (skip the next-prompt notification), return appropriate status.

**`builtin_bg(args, _out, shell) -> ExecOutcome`:**

1. If `!args.is_empty()`: print error, return `Continue(2)`.
2. `id = shell.jobs.current_stopped_id()`; if `None`, print `shuck: bg: no current job`, return `Continue(1)`.
3. Look up the job; capture `pgid`, `command.clone()`.
4. `unsafe { libc::killpg(pgid, libc::SIGCONT) }`.
5. Mark job state `Running`, `notified = true`.
6. Print `[N]+ <cmd> &` to stderr.
7. Return `Continue(0)`.

`bg` does not wait — the SIGCHLD reaper picks up the eventual exit and
prints the `Done` notification at the next prompt.

## 4. Signal disposition, `wait`, `jobs`

**`install_job_control_signals` (new, called from `Shell::run` after `Shell::new`):**

```rust
unsafe {
    libc::signal(libc::SIGTSTP, libc::SIG_IGN);
    libc::signal(libc::SIGTTIN, libc::SIG_IGN);
    libc::signal(libc::SIGTTOU, libc::SIG_IGN);
}
```

Why each:

- **SIGTSTP**: Ctrl-Z at an empty prompt would otherwise suspend shuck itself. With `SIG_IGN`, Ctrl-Z is delivered to whatever pgrp owns the terminal — during foreground execution that's the job, at the prompt it's no-op.
- **SIGTTOU**: `tcsetpgrp` from a non-foreground pgrp delivers SIGTTOU. Without `SIG_IGN`, shuck would suspend itself mid-handoff.
- **SIGTTIN**: defensive — shuck never reads `/dev/tty` directly today, but ignoring matches bash and avoids surprises.

The existing SIGINT handler (signal-hook flag register) stays. In
practice, SIGINT now rarely reaches shuck during execution because the
foreground job owns the terminal pgrp; at the prompt, rustyline handles
Ctrl-C internally.

The SIGCHLD reaper changes:

```rust
let r = unsafe { libc::waitpid(-1, &mut raw_status, libc::WNOHANG | libc::WUNTRACED) };
```

and adds the `WIFSTOPPED` branch described in §1.

**`Shell` struct gains:** `pub shell_pgid: i32`, captured in `Shell::new`
via `unsafe { libc::getpgrp() }`.

**`builtin_wait`:** today loops on `has_running()`. Renamed to
`has_pending()` (covers `Running` and `Stopped`). Stopped jobs do not
satisfy `wait` — the user must `fg`, `bg`, or kill them. (Bash semantics.)

**`builtin_jobs`:** unchanged structurally; gains `Stopped` rendering
through `render_state`. Width bump to 24.

## 5. Edge cases & error handling

| Scenario                                                  | Behavior                                                                                |
| --------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| `tcsetpgrp` ENOTTY (no tty, e.g., piped stdin)            | Silently skip both directions. Pgrp setup still happens. Ctrl-Z won't reach the job.    |
| `tcsetpgrp` EPERM (job pgrp already exited race)          | Skip; the post-wait `tcsetpgrp` to `shell_pgid` is unaffected.                          |
| `killpg(SIGCONT)` ESRCH (job already exited)              | Re-check current job; if gone, print "no current job" and return 1.                     |
| `fg` / `bg` with no current job                           | `shuck: <name>: no current job` to stderr, status 1.                                    |
| `fg` / `bg` with any arg                                  | `shuck: <name>: arguments not supported in this version`, status 2.                     |
| `bg` on a Running job                                     | "no current job" — `current_stopped_id` only returns Stopped jobs.                      |
| Pipeline where only some stages stop                      | Foreground waiter sees first stop, breaks; SIGCHLD reaper catches the rest.             |
| Ctrl-Z at the prompt                                      | `SIG_IGN` suppresses it; rustyline returns from `readline` with the partial line.       |
| Pipeline exits very fast (before `tcsetpgrp` runs)        | `tcsetpgrp` returns EPERM/ENOTTY → silently skipped; waitpid returns immediately.       |
| `wait` with one Stopped job and no Running                | Blocks forever (matches bash). User Ctrl-C interrupts (handled by SIGINT/rustyline).    |

## 6. Testing

Most of sub-project B can't be unit-tested cleanly — pgrp behavior,
SIGTSTP delivery, and `tcsetpgrp` all require a real controlling
terminal. The plan keeps unit tests focused on what's deterministic and
relies on a manual smoke-test checklist for the tty-bound paths.

**Unit tests (`src/jobs.rs`, `src/builtins.rs`):**

- `JobState::Stopped(SIGTSTP)` renders as `"Stopped"`.
- `JobState::Stopped(SIGTTIN)` renders as `"Stopped (tty input)"`.
- `JobState::Stopped(SIGTTOU)` renders as `"Stopped (tty output)"`.
- `JobState::Stopped(99)` renders as `"Stopped (signal 99)"`.
- `JobTable::current_id()` returns the `+` job across mixed Running/Stopped tables.
- `JobTable::current_stopped_id()` skips Running jobs.
- `has_pending()` returns true with one Stopped job and no Running.
- `builtin_fg` with empty table → status 1, correct stderr.
- `builtin_bg` with empty table → status 1, correct stderr.
- `builtin_fg`/`builtin_bg` with extra args → status 2, correct stderr.
- `builtin_bg` on a synthetic Stopped job: state transitions to Running, `[N]+` line printed.

**Manual smoke-test checklist (real pty required):**

1. `sleep 100`, press Ctrl-Z → `[1]+ Stopped sleep 100`, prompt returns.
2. `jobs` shows `[1]+  Stopped  sleep 100`.
3. `fg` → echoes `sleep 100`, blocks; Ctrl-Z stops again.
4. `bg` → `[1]+ sleep 100 &`, prompt returns.
5. `jobs` shows `[1]+  Running  sleep 100`.
6. Wait until sleep finishes; next prompt shows `[1]+ Done sleep 100 &`.
7. `vim /tmp/x` — vim takes the terminal cleanly, Ctrl-Z suspends it, `fg` resumes.
8. `cat &` → backgrounded; immediately stops with `[1]+ Stopped (tty input) cat`.
9. `wait` with a Stopped job blocks; Ctrl-C interrupts.
10. Ctrl-Z at empty prompt does nothing.

## 7. File summary

| File                    | Changes                                                                        |
| ----------------------- | ------------------------------------------------------------------------------ |
| `src/jobs.rs`           | `Stopped(i32)` variant, `current_id`, `current_stopped_id`, `has_pending`, `render_state` extension, column width 24, reaper uses `WUNTRACED`. |
| `src/shell.rs`          | `install_job_control_signals`; capture `shell_pgid` and pass to `Shell`.       |
| `src/shell_state.rs`    | `Shell.shell_pgid: i32` field.                                                 |
| `src/executor.rs`       | Foreground pipelines spawn in own pgrp; `give_terminal_to` / `take_terminal_back` helpers; `WUNTRACED` wait; on stop, register Job in `Stopped` state. |
| `src/builtins.rs`       | New `builtin_fg`, `builtin_bg`. Update `is_builtin` to include them.           |

No new dependencies. `libc` already provides `tcsetpgrp`, `WUNTRACED`,
`WIFSTOPPED`, `WSTOPSIG`, `killpg`, `signal`, `getpgrp`, `SIGCONT`,
`SIGTSTP`, `SIGTTIN`, `SIGTTOU`, `SIG_IGN`.

## 8. Out of scope / future work

- **Sub-project C**: `%`-specs for `fg`/`bg`/`wait`/`kill`, `kill` builtin, `disown`.
- **Backgrounded multi-pipeline sequences** (`cmd1 && cmd2 &`): still deferred, needs a real subshell primitive.
- **`set -m`/`+m`**: not relevant; shuck is always interactive.
- **`suspend` builtin**: out of scope.
- **`WCONTINUED` notifications**: bash doesn't print a notification when SIGCONT resumes a job, so we don't either.
