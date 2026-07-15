# Pipeline-stage spawn failure: child-side diagnostic + non-abort — Design

**Issue:** [#78](https://github.com/jdstanhope/huck/issues/78) (`echo hi | nosuchcmd 2>&1` — huck
prints raw `No such file or directory` to the real stderr and aborts the pipeline; bash prints
`nosuchcmd: command not found` to the stage's redirected fd 2, exits that stage 127, and keeps the
pipeline running).

## Problem

When an external pipeline stage's program can't be run, huck diverges from bash 5.2.21 in **four**
ways (all verified). For `echo hi | nosuchcmd`:

| aspect | bash | huck (current) |
|---|---|---|
| message | `nosuchcmd: command not found` | raw `No such file or directory` (no name) |
| destination | the stage's redirected fd 2 (`2>&1` capture, `2>file`) | the parent's real stderr |
| stage exit / `PIPESTATUS` | `127` (or `126`), full array (`0 127 0`) | rc 1, **empty** `PIPESTATUS` |
| pipeline | continues — every stage runs | **aborts** (`bail_teardown_pipeline`) |

Root cause: the failed-spawn branch in `spawn_pipeline`
(`crates/huck-engine/src/executor.rs`, the `Err(e)` arm after the stage dispatch) emits
`bash_io_error(&e)` (raw errno text, no command name) to `err_writer(err_sink, sink)` — the **parent's**
sink, which does not reflect the stage's redirects (those are applied in the child) — and then calls
`bail_teardown_pipeline`, tearing down the whole pipeline.

Correct exit codes are bash's convention: **127** = command not found; **126** = found but not
executable (permission denied, is-a-directory, etc.).

### Why the single-command path isn't the model

The single-command path (`run_subprocess`) emits in the **parent** via `emit_exec_spawn_diag`. That
routes to a `2>&1` capture (a special case) but **not** to `2>file` (bash writes the error into the
file; huck writes to real stderr), and it returns rc 1 instead of 126 for a non-executable file. So
"reach parity with the single-command path" would still not match bash. bash gets it right because the
**child** prints the diagnostic *after* applying its redirects — fd 2 already points at the file /
pipe / capture, so a plain `write(2, …)` routes correctly. This design mirrors bash's child-side emit.

The analogous single-command gaps (`2>file` routing, rc 126) live in a different code path
(`run_subprocess`) and are **out of scope** here — filed as a separate follow-on issue.

## Design

Detect an unrunnable external stage **before** spawning, and instead of the normal spawn fork a
**diagnostic child** that applies the stage's fds, writes the bash-formatted diagnostic to its own
fd 2, and `_exit`s 126/127. Because the child applies redirects first, the message routes to
`2>&1` / `2>file` / the downstream pipe exactly like a real stage, `PIPESTATUS` naturally records the
exit code, and the pipeline continues. This reuses the v296 per-stage-failure wiring
(`spawn_failed_stage`) — a stage that exits with a code, wired into the inter-stage pipes.

### Component 1 — runnability classification (new)

A helper (in `executor.rs`, near `classify_stage`) classifies a resolved program string:

```rust
enum StageRunnability {
    Runnable,
    /// `body` is the bash diagnostic body (e.g. "nosuchcmd: command not found");
    /// `code` is the stage exit status (127 not-found, 126 not-executable).
    NotRunnable { body: String, code: i32 },
}

fn classify_stage_runnability(program: &str, shell: &Shell) -> StageRunnability
```

Logic (mirrors bash's command search + `execve` outcomes):

- **`program` contains `/`** — stat it directly:
  - `metadata` errors `ENOENT` → `NotRunnable { "{program}: No such file or directory", 127 }`
  - `metadata` errors otherwise → `NotRunnable { "{program}: {bash_io_error(e)}", 126 }`
  - is a directory → `NotRunnable { "{program}: Is a directory", 126 }`
  - not executable (`libc::access(program, X_OK) != 0`) →
    `NotRunnable { "{program}: {bash_io_error(errno)}", 126 }` (`bash_io_error` of the real `access`
    errno — normally `Permission denied`)
  - else → `Runnable`
- **bare name** — PATH search via `builtins::search_path_for(program, shell)`:
  - `Some(_)` → `Runnable`
  - `None` → `NotRunnable { "{program}: command not found", 127 }`

`bash_io_error` (`crate::bash_io_error`) already maps an `io::Error`/errno to bash's `strerror`
text; the errno-text cases derive their message from a real `metadata`/`access` errno so it matches
libc exactly. The dir case is special-cased to `Is a directory` (bash reports `EISDIR` there even
though `execve` on a directory returns `EACCES`), matching the observed bash output.

### Component 2 — the diagnostic child (new)

```rust
fn spawn_command_error_stage(
    stdio: ChildStdio,
    pgid_target: i32,
    parent_fds_to_close: &[RawFd],
    replay_ops: Vec<ChildRedirOp>,   // the stage's ChildRedirPlan.ops
    held: Vec<OwnedFd>,              // plan.held; parent drops after fork
    diag: Vec<u8>,                   // full formatted "<prog>: line N: <body>\n"
    exit_code: i32,
) -> Result<i32, io::Error>
```

Modeled on `spawn_failed_stage`, extended to also install the stdio + redirects and emit. Child branch
(all async-signal-safe: `setpgid`/`dup2`/`close`/`write`/`_exit` only, no `OwnedFd` destructors — slots
are `into_raw()`d first, per the `child_fd` fork/`pre_exec` Drop-safety contract):

1. `if pgid_target >= 0 { setpgid(0, pgid_target) }`.
2. Install the `ChildStdio` onto fds 0/1/2. **Do not re-derive this** — `fork_and_run_in_subshell`'s
   child already does it correctly, including the fd-overlap PRE-MOVE pass (move any owned source in
   `0..=2` up to `>=3` so `dup2`'s source ≠ target and `FD_CLOEXEC` is cleared). Extract that
   stdio-install block into a shared `unsafe fn install_child_stdio(stdio: ChildStdio) -> [RawFd; 3]`
   (returns the post-move raw source fds) and call it from BOTH `fork_and_run_in_subshell` and
   `spawn_command_error_stage`. `Inherit` slots leave the shell's real fd in place.
3. `replay_redir_ops(&replay_ops)` — the stage's `2>&1` / `2>file` / fd>2 / close ops, in source order,
   AFTER the stdio install (so `2>&1` sees the piped/captured fd 1).
4. Close each `parent_fds_to_close` not claimed as a slot or a replay target (same exclusion
   `spawn_external_with_fds` uses, so a `dup2`-then-`close` can't defeat a redirect).
5. `write(2, diag.as_ptr(), diag.len())` — routes to wherever fd 2 now points.
6. `_exit(exit_code)`.

Parent branch: `drop(held)` right after fork (closes the parent's redirect-target copies; the child
inherited its own copies), `setpgid(pid, pgid_target)` (race-close), return `Ok(pid)`.

### Component 3 — wiring in `spawn_external_with_fds`

Insert the check inside `spawn_external_with_fds`, **after** `resolve(exec, …)` (so we have the
expanded `resolved.program` and `resolved.args`) and **before** the `ProcessCommand` is built (so
`stdio` and `plan` are still un-consumed):

```rust
match classify_stage_runnability(&resolved.program, shell) {
    StageRunnability::Runnable => { /* existing spawn path, unchanged */ }
    StageRunnability::NotRunnable { body, code } => {
        // Format the exact diagnostic bytes with the shell's prologue.
        let mut diag: Vec<u8> = Vec::new();
        crate::emit_error_to(shell, &mut diag, None, format_args!("{body}"));
        flush_stdout();
        return spawn_command_error_stage(
            stdio, pgid_target, parent_fds_to_close, plan.ops, plan.held, diag, code,
        );
    }
}
```

`emit_error_to(shell, &mut Vec<u8>, None, …)` produces the byte-identical `<prog>: line N: <body>\n`
prologue (same machinery every other diagnostic uses; `None` = the shell's current line number).
`spawn_external_with_fds` returns `Ok(pid)` for the diagnostic child, so the **caller
(`spawn_pipeline`) treats it as a normal forked stage — no code change at the call site, no abort**;
`stages.push(Forked(pid))` records it and the normal `waitpid` fills `PIPESTATUS[i]` with 126/127.

The xtrace emit (`set -x`) stays on the runnable path only (bash does not xtrace a not-found command).

## Error handling & residuals

- The pre-check covers the command-search + executability failures bash reports as 126/127 — the
  common cases. A genuine **`process.spawn()`** failure after a passing pre-check (a TOCTOU race, or an
  `ENOEXEC` text-with-exec-bit that bash would re-run via `/bin/sh`) is rare and **keeps current
  behavior** (the caller's existing `Err` → emit + `bail_teardown_pipeline`). Documented residual, not
  addressed here.
- `fork()` failure in `spawn_command_error_stage` returns `Err(io::Error)` → the caller's existing
  bail path (genuine infrastructure failure), matching `spawn_failed_stage`.

## Testing

New `tests/scripts/pipeline_stage_spawn_fail_diff_check.sh` — byte-identical rc + stdout + stderr +
`PIPESTATUS` vs bash 5.2.21, prologue-normalized (`bash:`/`huck:` → `SH:`). Cases:

1. last stage not found: `echo hi | nosuchcmd; echo "rc=$? ps=${PIPESTATUS[*]}"` → rc 127, `ps=0 127`.
2. middle stage not found (pipeline continues): `echo hi | nosuchcmd | cat; echo "ps=${PIPESTATUS[*]}"`
   → `ps=0 127 0`.
3. `2>&1` capture: `x=$(echo hi | nosuchcmd 2>&1); echo "cap=[$x] rc=$?"` → message in `cap`, rc 127.
4. `2>file`: `echo hi | nosuchcmd 2>FILE; cat FILE` → message in the file.
5. non-executable file → 126 `Permission denied` (a `chmod 644` temp file).
6. directory as command → 126 `Is a directory` (`/etc`).
7. slash-path not found → 127 `No such file or directory` (`/no/such/x`).
8. `pipefail`: `set -o pipefail; echo hi | nosuchcmd | cat; echo rc=$?` → rc 127.

If the pipeline-stage prologue line number diverges from bash, the harness normalizes `line N:` too
(and the gap is noted); cases 1–2/8 are the primary gates (rc + `PIPESTATUS` + non-abort). The
existing `pipeline_stage_redirect_fail_diff_check.sh` (v296) stays green. Engine lib + the full diff
sweep stay green on both binaries.

## Scope / non-goals

- **In scope:** external pipeline-stage spawn failure — correct message routed child-side, 126/127
  exit, `PIPESTATUS` populated, pipeline not aborted.
- **Out of scope (separate follow-on issue):** the single-command path's (`run_subprocess`) `2>file`
  routing and rc-126 gaps — same root idea, different code path.
- **Out of scope:** `ENOEXEC` (run-as-shell-script) and post-pre-check TOCTOU spawn failures (residual,
  documented).
- **Unaffected:** InProcess stages (builtins/functions/compounds) — they never reach
  `spawn_external_with_fds`.

The merged PR closes #78.
