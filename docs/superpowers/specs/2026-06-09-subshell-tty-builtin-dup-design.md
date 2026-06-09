# huck v124 — interactive subshell tty-deadlock + builtin `>&N` stdout redirect Design

**Status:** approved design, ready for implementation plan.
**Implements:** two independent bugs that together break `nvm ls` (and many real
`~/.bashrc`-class scripts), bundled under the goal *"make `nvm ls` work"*:
- **Bug A** — an interactive **foreground subshell** whose body does sustained
  tty I/O **deadlocks** (the user's hang; cannot Ctrl-C out).
- **Bug B** — **builtins ignore a `>&N` stdout redirect** (a correctness bug;
  surfaces as nvm printing every alias as `→ ∞`).
**Branch (impl):** `v124-subshell-tty-builtin-dup`.
**Why now:** user reports `nvm ls` hangs after sourcing `~/.bashrc`; bare `nvm`
works. Both bugs were root-caused this session (Phase-1 investigation complete;
see "Evidence" below).

## Evidence (probed this session)

Constraint honored: never sourced the user's `~/.bashrc` (it holds `PG*` creds);
reproduced via `~/.nvm/nvm.sh` directly + a headless PTY harness.

### Bug A — subshell tty deadlock (interactive-only)
- PTY repro (huck in a `pty.fork`'d session = controlling tty):
  - `( command ls -1qA /usr/bin | cat )` → **HANG**
  - `( command ls -1qA /usr/bin | grep -q . )` → **HANG** (nvm's exact form)
  - `( command ls -1qA /usr/bin | head -1 )` → **HANG**
  - `( echo hi | cat )`, `( true | true )` → ok (output too small to fill the pipe)
  - bare `command ls … | grep -q .` (no subshell) → ok, but prints
    `[1]+ Stopped (tty output)` then recovers
- Non-interactive (no tty): `( command ls … | cat )` / `… | grep -q .`
  **complete** (rc 0). ⇒ purely a job-control/terminal bug, not a pipe/fd bug.
- Trigger: a **subshell-wrapped pipeline whose first stage fills the pipe buffer**
  (so the reader must keep draining while the writer is still producing → the
  body needs the terminal). nvm hits it at `nvm.sh:1485`
  `if ! [ -d "$DIR" ] || ! (command ls -1qA "$DIR" | nvm_grep -q .); then`.

**Root cause (in code):** `run_command`'s `Command::Subshell` arm
(`executor.rs:305-374`) forks the body with `fork_and_run_in_subshell(… ,
pgid_target = 0, …)`. The child (`fork_and_run_in_subshell`, `executor.rs:4349+`)
resets `SIGTTOU/SIGTTIN/SIGTSTP` to `SIG_DFL` and `setpgid(0,0)` → its **own new
(background) process group**. But the parent **never hands that pgroup the
terminal** (no `give_terminal_to`/`tcsetpgrp` — unlike the pipeline path at
`executor.rs:3060-3113` and `3072`) and waits with plain `waitpid(pid, …, 0)`
(no `WUNTRACED`). So the subshell pgroup is a background group; when its pipeline
does sustained tty I/O it is stopped (SIGTTOU), and the parent's non-`WUNTRACED`
wait never wakes → **deadlock**.

### Bug B — builtins ignore `>&N` on stdout (always reproducible)
- Minimal: `a=$(echo Z >&2); echo "[$a]"` → **bash `[]`**, **huck `[Z]`**.
  Also `printf '%s\n' Z >&2`, and inside groups/subshells/functions.
- External commands are correct: `a=$(/bin/echo Z >&2)` → huck `[]`. So the bug
  is **builtin-specific**.
- nvm chain: `nvm_err () { >&2 nvm_echo "$@"; }`, and `nvm_echo` runs the
  **`printf` builtin**. `nvm_alias "v24.16.0"` (a leaf, no alias file) calls
  `nvm_err 'Alias does not exist.'`. With Bug B that text goes to **stdout**, so
  in `ALIAS_TEMP="$( (nvm_alias "$ALIAS" 2>/dev/null | head|tail) )"` it leaks
  past `2>/dev/null` into the capture → the resolution loop keeps going, re-feeds
  the message, repeats, and trips nvm's own cycle sentinel `ALIAS="∞"`
  (`nvm.sh` `nvm_resolve_alias`). Result: every alias prints `→ ∞` (verified:
  non-interactive `nvm alias` completes rc 0 but shows `→ ∞` for all).

**Root cause (in code):** `open_stage_files`/`resolve` map a stdout
`Redirect::Dup` to `files.stdout = None` (`executor.rs:2188`, comment *"callers
check exec.stdout for Dup and apply dup2"*). The **external/fork** path honors it
via `stdout_dup_target` + pre_exec `dup2`. But the **builtin** branches in
`run_exec_single` (`executor.rs:~2828` control-builtins, `~2885` regular
builtins) never consult `cmd.stdout` for a `Dup`; with `files.stdout == None`
they just write to the `StdoutSink` (the capture buffer, or real stdout). Stderr
has the symmetric handling already (`prepare_builtin_stderr` + `dup_target` for
`2>&1`, `executor.rs:2343`/`2872`); stdout `>&N` has **no** equivalent.

## Architecture

Two independent, localized fixes in `src/executor.rs`. No AST/lexer/parser
changes. Bug A touches the subshell-command execution + (re)uses the existing
job-control helpers; Bug B touches the in-process builtin redirect setup.

### Fix A — make an interactive foreground subshell a proper job

In `run_command`'s `Command::Subshell` arm, compute
`let interactive = matches!(sink, StdoutSink::Terminal) && !shell.in_subshell && !shell.in_completion;`
(identical to `run_multi_stage:3200`) and branch:

- **Not interactive** — a Capture sink (`$( ( … ) )`), a **nested** subshell
  (`in_subshell == true`), or a completion context: **unchanged** — keep the
  existing pipe-creation, `io::copy` drain, and plain `waitpid(pid, 0)`. (No
  terminal handoff: either there is no controlling terminal in play, or an
  enclosing subshell/pipeline already owns it. This is why nested
  `( ( … ) )` and capture contexts must NOT re-run the dance.)
- **Interactive** (`interactive == true` — a standalone foreground subshell at
  the Terminal sink): after
  `fork_and_run_in_subshell(cmd, shell, STDIN, STDOUT_FILENO, STDERR, 0, …)`,
  perform the same dance the single-command/pipeline interactive path uses
  (template: `executor.rs:3060-3113`):
  1. Race-close `setpgid(pid, pid)` (ignore `ESRCH`/`EACCES`).
  2. `give_terminal_to(pid)`.
  3. `wait_with_untraced(pid)` (`executor.rs:4308+`).
  4. On **stopped** (`(status, true)`): register a job
     (`shell.jobs.add(pid, vec![pid], label)`), set
     `JobState::Stopped(WSTOPSIG)`, `notified=true`, print the `\n{notification_line}`,
     `std::mem::forget`-equivalent cleanup, return `128 + sig`. Label = the
     subshell's source text if readily available, else the literal `"( subshell )"`.
  5. On **exited/signaled**: compute status as today (WEXITSTATUS / 128+WTERMSIG).
  6. On **error**: status 1.
  7. **Always** reclaim: `give_terminal_to(shell.shell_pgid)`.
  Set `PIPESTATUS` to the single resulting code (as today).

Gating: treat the Terminal sink as the interactive branch. `give_terminal_to`
is already a safe no-op when there is no controlling tty (`tcsetpgrp` fails
silently — see `give_terminal_to`, `executor.rs:4302`), so a non-interactive
`huck script.sh` run (Terminal sink, no tty) takes this branch harmlessly:
`setpgid` + no-op `give_terminal_to` + `WUNTRACED` wait + no-op reclaim, with no
SIGTTOU possible → identical observable behavior to today for scripts.

Notes:
- The child keeps `setpgid(0,0)` (its own pgroup) — bash-faithful, so
  Ctrl-C/Ctrl-Z hit the subshell, not huck. Do **not** join the shell's pgroup.
- v108's `in_subshell` flag (which disables the **inner** pipeline's own job
  control inside the subshell child) is unchanged and still required.
- Only the **standalone** `Command::Subshell` arm is affected. A subshell used as
  a **pipeline stage** (`( … ) | cmd`) already runs through `run_multi_stage`
  with the pipeline's `pgid_target` and terminal handoff; a subshell in a
  **capture** is the unchanged branch above. No other entry points need changes.

### Fix B — honor `>&N` stdout redirect for builtins

In `run_exec_single`, before the two builtin execution branches, compute a
builtin-stdout File for a stdout `Dup` and route the builtin's output to it.

New helper (free fn in `executor.rs`):
```text
/// For a builtin with a stdout `Dup` redirect (`>&N`), produce the File the
/// builtin should write to. `>&1` → None (write to the normal sink). `>&N`
/// (N != 1) → Some(File) dup'd from fd N. `>&-` (close) → Some(File) on
/// /dev/null (output discarded). Resolves N via resolve_fd_target.
fn builtin_stdout_dup_file(cmd: &ExecCommand, shell: &mut Shell)
    -> Result<Option<File>, ()>
```
- Use `resolve_fd_target(source, shell)` to get N (already used by the external
  path). On a bad fd, print `huck: <err>` and return `Err(())` (status 1),
  matching the external path's error handling.
- `>&-` is `Redirect::Dup { source }` where `source` expands to `-`; treat as
  close → open `/dev/null` for write.
- Build the File with `dup(N)` via `libc::dup` + `File::from_raw_fd` so the
  File owns an independent fd that closes on drop (does not close the real fd N).

Wire-in: in the regular-builtin branch (`~2885`) and the control-builtin branch
(`~2828`), when `files.stdout.is_none()` and `cmd.stdout` is a `Dup`, set the
builtin output to this File (i.e. take the existing `Some(file) => run_builtin(…
&mut file …)` path). When the helper returns `None` (`>&1` / not a Dup), keep the
current sink behavior. The declaration-builtin sub-cases route the same way.

Interaction with the existing `2>&1` handling: `stderr_dups_to_stdout` inspects
`files.stdout` to decide the `2>&1` dup target; once a stdout `Dup` yields a
`Some(File)`, that logic continues to work (it dups the file's fd). The
pathological `>&2` + `2>&1` same-command combo remains source-order-insensitive
(pre-existing **L-08**, intentional) — note, do not try to fix here.

Scope of N: `>&0/1/2/N` for small integer N (what `resolve_fd_target` already
parses). `>&-` close. This matches the external path's capabilities.

## Scope & correctness
- **Fix A**: zero change to non-interactive scripts (no-op terminal handoff) and
  to capture-context subshells (unchanged branch). Only an interactive,
  terminal-owning standalone subshell changes — it stops deadlocking.
- **Fix B**: external commands unchanged (already correct). Builtins without a
  stdout `Dup` unchanged (sink path). Only `>&N`-on-builtin changes.
- Neither fix alters the AST or any other redirect form (`>file`, `>>`, `2>`,
  `2>&1`, heredoc, here-string, `<`).

## Must-not-regress
- Non-interactive `huck script.sh` subshell behavior (status, output, PIPESTATUS).
- `$( ( … ) )` command-substitution-of-subshell capture (the Capture branch).
- Existing pipeline job control (Ctrl-Z/fg/bg, the v108/v121 PTY suites).
- `2>&1`/`2>file` on builtins (incl. the L-25 capture residual — unchanged).
- `>&2`/`2>&1` on external commands.
- Full unit + integration suites; all bash-diff harnesses; clippy `--all-targets`.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | Fix A: interactive branch in the `Command::Subshell` arm (mirror the 3060-3113 dance; reclaim terminal). Fix B: `builtin_stdout_dup_file` helper + wire into the two builtin branches. Unit tests. |
| `tests/subshell_tty_pty.rs` (NEW) | PTY regression: `( command ls -1qA /usr/bin \| cat )` returns promptly (not hung); a small subshell still works; skips gracefully without a PTY (mirror `completion_jobcontrol_pty.rs`). |
| `tests/builtin_stdout_dup_integration.rs` (NEW) | `>&2`/`>&1`/`>&-` on `echo`/`printf` in capture + file forms vs bash. |
| `tests/scripts/builtin_stdout_dup_diff_check.sh` (NEW) | 47th bash-diff harness, byte-identical. |
| `docs/bash-divergences.md` | Add then resolve: Bug B is currently **undocumented**; add a brief note only if a residual remains (e.g. L-08 already covers the `>&2 2>&1` order case). Bug A (subshell tty deadlock) is undocumented — no entry needed once fixed. If any follow-on gap is found, add a `[deferred]` entry. |
| `README.md` | Bump harness count 46 → 47. |

## Testing
1. **Unit (Fix B)** in `executor.rs`: `builtin_stdout_dup_file` resolves `>&2`
   to a File on the current fd 2; `>&1` → None; bad fd → Err. Plus an
   integration-level check that `$(echo Z >&2)` captures empty.
2. **Integration (Fix B)** vs bash (file-arg per L-27): the minimal repros —
   `a=$(echo Z >&2)`, `printf … >&2`, `{ echo Z >&2; } 2>/dev/null` capture,
   `echo Z >&2 1>/tmp/f` (goes to fd2/terminal not the file), `>&-` discards.
   Byte-identical stdout + the captured value.
3. **47th bash-diff harness** `builtin_stdout_dup_diff_check.sh` — ~6 fragments.
4. **PTY (Fix A)** `tests/subshell_tty_pty.rs` (expectrl, mirror
   `completion_jobcontrol_pty.rs`): drive `( command ls -1qA /usr/bin | cat )`
   and assert a prompt returns within a few seconds (would hang pre-fix); assert
   `( echo hi | cat )` prints `hi`. Skip when no PTY.
5. **Regression**: full suite, all harnesses, `cargo clippy --all-targets`; run
   the existing `pty_interactive` + `subshell_pipeline_pty` + completion PTY
   suites (must stay green — job-control behavior).
6. **Payoff (PTY)**: source `~/.nvm/nvm.sh`, run `nvm ls`; it must (a) **not
   hang** (Fix A) and (b) show real versions in the alias section, **no `→ ∞`**
   (Fix B). Report before/after honestly.

## Edge cases & notes
- **Stopped subshell label**: a subshell has no single program name; use the
  source text if cheaply available, else `"( subshell )"`. Cosmetic only.
- **`give_terminal_to` without a tty**: already a silent no-op — keep relying on
  that so the interactive branch is safe in scripts (no separate `isatty` gate
  needed; if one is cleaner, gate on the existing `interactive` notion).
- **Fix B `>&-`**: routing to `/dev/null` matches the observable "output
  discarded"; bash technically closes fd 1 (a builtin writing to a closed fd
  gets EBADF) — `/dev/null` is the faithful-enough, panic-free choice.
- **Two fixes, one branch**: bundled by the user under the `nvm ls` goal; they
  are in different subsystems but share the verification target. Keep the two
  implementations in clearly separate tasks/commits.
