# v287 — Async command stdin defaults to `/dev/null` (not the terminal)

**Issue:** [#126](https://github.com/jdstanhope/huck/issues/126) — a backgrounded
command with no input redirection must get `/dev/null` as stdin, per POSIX/bash,
so it cannot steal input from the terminal and block forever. `divergence` +
`bug` + `sev:medium`.

This is the FIRST hang in the bash test-suite `redir` category
(redir.tests:162, `/bin/cat & wait`), reached before the move-fd tests that
v286 (#121) fixed. So `redir` has (at least) two independent hangs; v286 fixed
the later one, this fixes the earlier one.

## Problem

An asynchronous (`&`) command with no explicit input redirection must have its
stdin default to `/dev/null` (opened `O_RDONLY`), per POSIX ("the standard input
for an asynchronous list, before any explicit redirections, shall be considered
to be the file `/dev/null`"). huck instead lets such a command inherit the
shell's stdin (the terminal / an open pipe), so a backgrounded reader blocks
forever:

```
/bin/cat & wait; echo "rc=$?"     # bash -> rc=0 (cat gets /dev/null, EOFs)
                                  # huck -> HANGS (cat inherits the terminal)
```

huck has two background entry points in `crates/huck-engine/src/executor.rs`:

- **Path A — `run_background_sequence(pipeline)`** — reached by a trailing `&`
  on a `Command::Pipeline`. A lone `cmd &` is wrapped by the parser as a
  1-stage pipeline and lands here. It already defaults stage-0 stdin to
  `/dev/null`, so a bare `cmd &` at EOF is already correct.
- **Path B — `run_background_subshell(cmd)`** — reached by `(cmd) &`, a
  collapsed `a && b &` / `a || b &` group, and **every** separator-group
  background carved out by `execute_sequence_body` (e.g. `cmd & wait`,
  `cmd & cmd2`). It passes `libc::STDIN_FILENO` (the terminal) unconditionally,
  with a comment wrongly claiming `(cmd) &` inherits the terminal. This is the
  #126 bug; the repro `/bin/cat & wait` (where `&` is a separator) hits it.

**Scope:** this iteration changes **Path B only** — the entire #126 hang lives
there. Path A has two latent divergences of its own ((a) it applies `/dev/null`
even in interactive shells; (b) it gives a backgrounded multi-stage pipeline's
first stage `/dev/null` instead of the inherited stdin), but both are masked by
a separate, larger bug: a bare trailing-`&` background child does not survive a
`huck -c` exit ([#128](https://github.com/jdstanhope/huck/issues/128)), so
Path A's behavior is not deterministically testable until #128 is fixed. The
Path A consistency fix is therefore deferred to
[#129](https://github.com/jdstanhope/huck/issues/129) (blocked by #128). The
unified rule below is stated in full for reference, but only Path B is wired up
here.

## Ground truth (bash 5.2.21, verified by `readlink /proc/self/fd/0`)

Shell stdin redirected from a real file so inheritance is observable.

**Non-interactive** — the `/dev/null` default applies to every async unit
**except a bare (top-level) multi-stage pipeline**, whose first stage inherits:

| async form | async child fd0 |
|---|---|
| `cmd &` (simple) | `/dev/null` |
| `(cmd) &` (subshell) | `/dev/null` |
| `{ cmd; } &` (brace group) | `/dev/null` |
| `a && b &` (and-or list) | `/dev/null` |
| `cmd < file &` (explicit input redirect) | the file (overrides) |
| `a \| b &` (bare pipeline, stage 0) | **inherited** (not `/dev/null`) |
| `a \| b \| c &` (bare pipeline, stage 0) | **inherited** |
| `(a \| b) &` (pipeline wrapped in subshell) | `/dev/null` |
| `{ a \| b; } &` (pipeline wrapped in group) | `/dev/null` |
| `true && (a \| b) &` | `/dev/null` |

Wrapping a pipeline in `(…)` / `{…}` / an and-or list flips its stage-0 stdin
back to `/dev/null`; only a **bare** top-level pipeline inherits.

**Interactive** (job control on, verified via a `script` pty): async units
always inherit the terminal — the `/dev/null` default is **not** applied.

## Design

### The unified rule

An async unit **inherits** the shell's stdin (fd 0) iff:

```
inherit_stdin == (is a bare multi-stage pipeline) || shell.is_interactive
```

Otherwise its stdin defaults to `/dev/null` (`O_RDONLY`). An explicit input
redirection on the unit overrides either default — no special-casing needed,
because the child applies its own redirects on top of the default stdin it is
handed (this is why `cmd < file &` already works today).

### Shared helper

One helper carries the rule so it lives in exactly one place
(`executor.rs`, alongside the two background functions):

```rust
enum AsyncStdin {
    /// Inherit the shell's stdin (fd 0).
    Inherit,
    /// A freshly opened /dev/null fd; the caller closes it after forking.
    DevNull(RawFd),
}

fn async_default_stdin(
    inherit: bool,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<AsyncStdin, ()> {
    if inherit || shell.is_interactive {
        return Ok(AsyncStdin::Inherit);
    }
    // File::open("/dev/null") -> AsyncStdin::DevNull(fd.into_raw_fd())
    // on error: emit "/dev/null: <bash_io_error>" to the err writer, return Err(())
}
```

`shell.is_interactive` is the interactivity gate (matches bash's
`interactive_shell == 0` guard); job control is orthogonal and not consulted for
this decision.

### Path B — `run_background_subshell`

Add an `inherit_stdin: bool` parameter. In the body:

```rust
let stdin_fd = match async_default_stdin(inherit_stdin, shell, sink, err_sink) {
    Ok(AsyncStdin::Inherit) => libc::STDIN_FILENO,
    Ok(AsyncStdin::DevNull(fd)) => fd,
    Err(()) => return ExecOutcome::Continue(1),
};
let fork_result = fork_and_run_in_subshell(
    cmd, shell, stdin_fd, libc::STDOUT_FILENO, libc::STDERR_FILENO,
    /*pgid_target=*/ if job_control { 0 } else { NO_PGROUP },
    /*parent_fds_to_close=*/ &[], None, None,
);
if stdin_fd != libc::STDIN_FILENO {
    unsafe { libc::close(stdin_fd); }   // parent drops its /dev/null copy
}
// ... existing Ok(pid)/Err(e) handling on fork_result ...
```

The stale "Inherit stdin from the terminal … match bash/dash for `(cmd) &`"
comment is replaced with one describing the actual rule.

Its three call sites pass `inherit_stdin`:

- `(cmd) &` explicit subshell (fast path) → `false`
- collapsed `a && b &` / `a || b &` and-or group (fast path) → `false`
- separator-group background in `execute_sequence_body` →
  `group.rest.is_empty() && matches!(group.first, Command::Pipeline(p) if p.commands.len() > 1)`

Only the separator-group site can present a bare multi-stage pipeline; the other
two always wrap a non-pipeline (subshell / and-or), so they pass `false`. The
predicate is computed at the call site, where the group structure is still
visible — it cannot be recovered from the already-wrapped `Command::Subshell`
that `run_background_subshell` receives.

### Path A — `run_background_sequence` (deferred, not changed here)

Path A already defaults stage-0 stdin to `/dev/null`, so a bare `cmd &` at EOF
is already correct for the common single-stage non-interactive case. Its two
divergences (interactive over-application; multi-stage pipeline stage-0) are
deferred to [#129](https://github.com/jdstanhope/huck/issues/129) because they
are not deterministically testable until the trailing-`&` child-survival bug
([#128](https://github.com/jdstanhope/huck/issues/128)) is fixed. The
`async_default_stdin` helper is written so #129 can reuse it verbatim
(`async_default_stdin(n > 1, …)`), but this iteration leaves Path A untouched.

### Out of scope

- **Path A (`run_background_sequence`) changes** — deferred to #129 (blocked by
  #128). See above.
- **The trailing-`&` child-survival bug** ([#128](https://github.com/jdstanhope/huck/issues/128))
  — a separate, larger divergence discovered during this work; not fixed here.
- **Interactive job-control terminal handling** beyond the stdin default (SIGTTIN
  stop semantics for background readers) — huck's existing behavior is unchanged.
- **`coproc`** and any other construct — only the `run_background_subshell`
  background path changes.

## Testing

### Diff harness `tests/scripts/async_stdin_diff_check.sh` (primary)

Byte-identical bash↔huck. Each fragment runs with the shell's own stdin
redirected from a fixture file and the async child prints
`readlink /proc/self/fd/0`, so every case emits a deterministic fd0 identity —
`/dev/null` for the defaulted cases, the fixture path for the inherited cases —
comparable with no timing dependence:

| fragment (run `< fixtureA`) | expected fd0 (both shells) |
|---|---|
| `readlink … & wait` (simple; the #126 form) | `/dev/null` |
| `(readlink …) & wait` | `/dev/null` |
| `{ readlink …; } & wait` | `/dev/null` |
| `true && readlink … & wait` | `/dev/null` |
| `readlink … < fixtureB & wait` (explicit redirect) | fixtureB path |
| `readlink … \| cat & wait` (bare pipeline) | fixtureA path (inherit) |
| `(readlink … \| cat) & wait` (subshell-wrapped pipeline) | `/dev/null` |
| `readlink … \| cat \| cat & wait` (3-stage bare pipeline) | fixtureA path (inherit) |

Plus a **functional anti-hang guard** in the same harness: `/bin/cat & wait;
echo "rc=$?"` fed from a still-open pipe carrying data, under a short `timeout`;
both shells print `rc=0` (cat EOFs on `/dev/null`). This is the direct #126
regression — it hangs on today's huck.

### Manual: interactive gate

A tty-backed diff harness is flaky, so verify by hand via a `script` pty that
`bash -i` and `huck -i` both let an async command inherit the terminal (fd0 is
the terminal, not `/dev/null`). Record the result in the plan; not automated.

### Regression

- Full `tests/scripts/run_diff_checks.sh` sweep stays green.
- `cargo test -p huck-engine` and `-p huck-syntax` (per-crate, single-threaded).
- Run the bash-suite `redir` category through its real runner (cd into the
  tests dir first, per the v286 lesson) to confirm the `/bin/cat & wait` hang at
  redir.tests:162 is cleared. Report honestly whether `redir` now passes or
  advances to a new blocker.

## Non-goals

- Interactive SIGTTIN/terminal-control semantics for background jobs.
- `coproc` stdin handling.
- Any change to foreground command stdin.
