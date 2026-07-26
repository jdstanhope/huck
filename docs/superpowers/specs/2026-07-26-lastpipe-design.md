# v338 — Implement `lastpipe` (flip the `lastpipe` bash-suite category)

Issue: [#306](https://github.com/jdstanhope/huck/issues/306) — `shopt lastpipe`
not implemented; the last pipeline stage always forks.

## Problem

huck accepts `shopt -s lastpipe` but ignores it — every stage of a multi-stage
pipeline forks (`run_multi_stage`/`spawn_pipeline`, `executor.rs`). So the last
stage's variable assignments never persist and its status/PIPESTATUS come from a
child:

```
shopt -s lastpipe; set +m
echo a b c | read foo; echo "foo=$foo"                       # bash: a b c  huck: (empty)
printf "%d\n" 1 2 3 | while read n; do tot=$((tot+n)); done  # bash: 6      huck: 0
```

bash: when `lastpipe` is enabled AND job control is off, the **last** command of
a pipeline runs in the **current shell environment** (not a subshell) — its side
effects (variable/array assignments) persist, and its exit status / control flow
apply to the shell. This is the complete residual of the `lastpipe` bash-suite
category. Implementing it flips `lastpipe` to PASS (Summary PASS 25→26, FAIL 57→56).

### What the category requires (from `lastpipe.tests` + `lastpipe{1,2,3}.sub`)

- **Variable persistence**: `echo a b c | read foo` → `$foo` set; `… | while read
  foo; do tot+=$foo; done` → `$tot` accumulates; `echo g h i | bar=7` → `$bar`.
- **Exit status / PIPESTATUS / pipefail**: `exit 142 | false` → `$?` and
  `${PIPESTATUS[@]}` reflect the in-process last stage; pipefail picks the
  rightmost non-zero.
- **Control flow from the last stage propagates** (lastpipe1): `exit 142 | exit
  14` → the `exit 14` exits the calling shell (script returns 14). (lastpipe2):
  a **function** last stage `cat | read var; return 42` → `$var` persists, `$?`
  = 42.
- **Compound / function / nested last stage** (lastpipe2): the last stage can be
  a `while` loop or a function whose body is itself a lastpipe pipeline.
- **Closed fd 0** (lastpipe3): `exec 0<&-; echo x | read x` → the pipe is dup'd
  onto fd 0 for the last stage regardless.

## Design

All in `crates/huck-engine/src/executor.rs`. Only foreground multi-stage
pipelines are affected; single-stage (already runs in the parent), background
(`&`), and job-control-on pipelines are unchanged.

### Gate

lastpipe takes effect iff:

```rust
let lastpipe = shell.shopt_options.get("lastpipe").unwrap_or(false)
    && !shell.job_control_active()
    && matches!(sink, StdoutSink::Terminal);
```

- `job_control_active()` (shell_state.rs:1204) false → non-interactive / `set +m`.
- `Terminal` sink → not inside `$()` (capture). Capture-context lastpipe is an
  explicit **follow-up**, not v338 (it interacts with the capture-drain loop and
  is observably harmless: a `$()` subshell's vars are discarded regardless).

### `PipelineStage::InProcess`

Add a variant to `PipelineStage` (executor.rs:6215), today only `Forked(i32)`:

```rust
enum PipelineStage {
    Forked(i32),
    InProcess { stdin_fd: RawFd }, // lastpipe: the last stage runs in the parent
}
```

The single-variant destructures (`stage_pids` reconstruction executor.rs:7368;
`wait_pipeline_raw` executor.rs:7505/7585) become compile-enforced touch points —
the in-process slot is skipped in the wait and carries no pid.

### `spawn_pipeline`: don't fork the last stage under lastpipe

In the stage loop, when `is_last && lastpipe`, build the last stage's stdin fd
exactly as today (its `prev_pipe_read`, honoring an explicit `<`/heredoc/herestr
override) but instead of spawning, push `PipelineStage::InProcess { stdin_fd }`
and preserve that fd out of the parent bulk-close (executor.rs:7320). Its stdout
is the terminal (fd 1, inherited) — no capture pipe (Terminal gate).

### `run_multi_stage`: run the in-process stage BEFORE the wait, then assemble

Critical ordering: the in-process last stage must run **before** reaping the
forked stages. Reaping first would let the upstream stages block writing to a
full pipe with no reader → deadlock.

```
let sp = spawn_pipeline(...)?;
// 1. If the last stage is InProcess, run it in the parent NOW (drains the pipe):
let inproc = if let Some(stdin_fd) = last_inprocess_stdin(&sp.stages) {
    let mut scope = RedirectScope::new();
    scope.redirect(shell, stdin_fd, libc::STDIN_FILENO, sink, err_sink)?; // dup2 pipe→fd0
    let outcome = run_command(&commands[n-1], shell, sink, err_sink);
    drop(scope);                     // restores fd 0 (handles a closed fd 0)
    close(stdin_fd);
    Some(outcome)
} else { None };
// 2. Wait the forked stages (in order), collect statuses.
// 3. Assemble PIPESTATUS = [forked statuses…, inproc status]; write via set_pipestatus.
//    Apply pipefail (rightmost non-zero) or last-stage rule — same as executor.rs:7463-7469.
// 4. Return:
//    - if `inproc` outcome is a control-flow variant (Exit / FunctionReturn /
//      LoopBreak / LoopContinue) → return it (propagates `exit 14`, `return 42`).
//    - else ExecOutcome::Continue(status).
```

`wait_pipeline_raw` skips the `InProcess` slot (no `waitpid`) and slots the
recorded in-process status in its place. The full-array `set_pipestatus` write
happens after the in-process stage runs, so any nested PIPESTATUS the in-process
stage set (e.g. an inner `cat | read var`) is correctly overwritten by the outer
pipeline's array.

Because the in-process stage runs through `run_command`, a **function**, a
**`while`/compound**, and **nested** lastpipe pipelines all work without extra
code (a nested pipeline re-enters `run_multi_stage` with its own lastpipe gate).

### fd-0 handling (lastpipe3)

`RedirectScope::redirect` (executor.rs:1015) `dup`s the target for restore then
`dup2`s the new fd onto it; it already handles an **unopened** target fd (restore
just closes it). So `exec 0<&-; echo x | read x` works: fd 0 is closed, the scope
dup2's the pipe onto fd 0, `read` reads it, and drop closes fd 0 again.

## Testing

Gate = bash 5.2.21 fidelity + `lastpipe` at 0 diff + no per-category regressions.

1. **Bash-diff harness** `tests/scripts/lastpipe_diff_check.sh` (new), byte-identical
   incl. exit: `read`-last-stage persistence; `while read` accumulation; `bar=7`
   assignment last stage; `exit N | exit M` shell-exit (run as `bash -c`/`huck -c`
   comparing `$?`); a **function** last stage with `return`; **nested** lastpipe;
   `${PIPESTATUS[@]}` for 2- and 3-stage pipelines incl. pipefail; closed-fd-0
   (`exec 0<&-; echo x | read x`); and the negative controls — lastpipe OFF (no
   persistence), and inside `$()` (suppressed, matches bash's discarded-subshell
   result for the tested shapes).
2. **`lastpipe` category** flips: `HUCK_BASH_TEST_CATEGORY=lastpipe` → PASS, 0 diff.
3. **Regression**: huck-engine lib green; the pipeline / job-control / redirect
   `-p huck` integration bins green (`subshell_pipeline*`, `pipefail`,
   `captured_pipeline_drain`, `builtin_pipe_flush`, `read`, `jobs_*`,
   `bg_pipeline_line_number`, the `*_pty` job-control bins); full `run_diff_checks.sh`
   sweep green; previously-flipped categories stay PASS; compare per-category
   diff-LINE counts vs the saved baseline (the pipeline-exec change touches a
   shared core — watch `read`, `dollars`, `jobs`, `posixpipe`, `set-e`, and any
   pipeline-heavy category). Run the soak harness is NOT required, but confirm no
   new fd leak in the pipeline path via the existing integration bins.

Per repo constraints: build the binary with `cargo build -p huck`; per-crate tests
single-threaded; NEVER `cargo test --workspace`; guard runner/sweeps with
`ulimit -v` + `timeout`; run the `-p huck` integration bins single-threaded before
push; NO GPL bash text copied.

## Scope

**In scope.** The lastpipe execution branch (gate, `PipelineStage::InProcess`,
in-process-before-wait run, PIPESTATUS/pipefail assembly, control-flow
propagation, closed-fd-0); the harness; the `lastpipe` flip; regressions.

**Out of scope (follow-up).** Capture-context lastpipe (inside `$()` — the
`Terminal` gate suppresses it); lastpipe under job control (bash also disables it
there). File a follow-up issue for capture-context if a future category needs it.

## Documentation

- Removes a divergence (no new intentional one). #306 auto-closes via the PR
  (`Closes #306`); `docs/bash-divergences.md` unchanged.
- Update `docs/bash-test-suite-baseline.md` (`lastpipe` PASS, Summary PASS 25→26,
  FAIL 57→56); record in `project_huck_iterations.md` + `MEMORY.md`.
