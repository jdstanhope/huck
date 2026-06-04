# huck v83 — `set -o pipefail` + `$PIPESTATUS` (M-50) Design

**Status:** approved design, ready for implementation plan.
**Closes:** M-50 (`set -o pipefail` and `$PIPESTATUS`) — currently `[deferred]` medium in `docs/bash-divergences.md`.
**Branch (impl):** `v83-pipefail` (created from `main` at plan time).

## Goal

- `set -o pipefail` / `set +o pipefail` (default off): when on, a pipeline's exit
  status is the **rightmost non-zero** stage status (or 0 if all stages
  succeeded), instead of just the last stage's status.
- `$PIPESTATUS`: an indexed array of the exit statuses of the stages of the
  **last executed simple-command pipeline** (each simple command is a
  one-element pipeline), updated after every leaf command.

## Background — verified bash 5.2 behavior

- `set -o pipefail` exit status = rightmost non-zero stage: `false|true`→1,
  `true|false`→1 (default off: `false|true`→0, last stage), `(exit 2)|(exit 3)`→3,
  `true|true`→0. `set -o` lists `pipefail off`. **No short flag** → `$-` unaffected.
- `$PIPESTATUS` is a normal indexed-array variable, **written at leaf execution
  sites** (verified):
  - simple command / builtin / **function call** → 1-element `[status]`
    (`false`→`(1)`, `echo hi`→`(0)`, `f(){ true|false; }; f`→`(1)` — a function
    call is opaque, its status only; `g(){return 5;}; g`→`(5)`).
  - multi-stage pipeline → per-stage vector (`true|false|true`→`(0 1 0)`).
  - subshell `(...)` → 1-element `[status]` (`(true|false)`→`(1)` — forked as one
    unit; the inner `(0 1)` stays in the child).
  - **compound commands `if`/`while`/`for`/`case`/`{ }` are TRANSPARENT** — they
    do not write `$PIPESTATUS`; their inner leaf pipelines do:
    `if false; then :; fi`→`(1)` (the condition), `for i in 1; do true|false; done`
    →`(0 1)`, `while false; do :; done`→`(1)`, `{ true|false; }`→`(0 1)`.
- `$PIPESTATUS` is readable/assignable but overwritten on the next leaf command
  (a user `PIPESTATUS=(9 9)` is clobbered by the next command).

## huck design

The infrastructure exists: `ShellOptions` (v69) for `set -o`; `VarValue::Indexed`
+ array expansion (v71) for the array; `wait_pipeline_raw` already collects the
full per-stage `Vec<Option<i32>>` (it just returns the last). The change is
threading per-stage statuses to a write site and adding the option.

### 1. `set -o pipefail` option (`src/shell_state.rs`, `src/builtins.rs`)
- Add `pub pipefail: bool` to `ShellOptions` (default `false`).
- Add `OptionInfo { name: "pipefail", short: None }` to `SHELL_OPTIONS`
  (`src/builtins.rs`); add `"pipefail" => Some(shell.shell_options.pipefail)` to
  `option_get` and `"pipefail" => { shell.shell_options.pipefail = value; Ok(()) }`
  to `option_set`. `set -o pipefail` / `set +o pipefail` toggle; `set -o` /
  `set +o` listing include it (driven by `SHELL_OPTIONS`). Because `short` is
  `None`, `dollar_dash_value` (`$-`) is unchanged — verify it skips short==None
  entries (it iterates option flags; pipefail must not appear in `$-`).

### 2. `$PIPESTATUS` write helper (`src/shell_state.rs`)
Add `pub fn set_pipestatus(&mut self, statuses: &[i32])` that writes a real
`PIPESTATUS` indexed-array variable (so all `${PIPESTATUS[@]}` / `${PIPESTATUS[N]}`
/ `${#PIPESTATUS[@]}` / `${!PIPESTATUS[@]}` forms work via existing machinery).
Build a `BTreeMap<usize, String>` from `statuses` (index→decimal string) and store
via the same path `replace_array` uses (a `VarValue::Indexed`), NOT readonly,
NOT exported. (Statuses are non-negative `i32`; store as decimal.)

### 3. Per-stage statuses + pipefail status (`src/executor.rs`)
- Change `wait_pipeline_raw` to surface the full per-stage vector. Cleanest:
  `PipelineWaitResult::AllExited(Vec<i32>)` (each slot's status, unfilled→1 as
  today). `run_multi_stage`, on `AllExited(stages)`:
  - `shell.set_pipestatus(&stages)`.
  - Compute the pipeline's scalar exit status: if `shell.shell_options.pipefail`,
    `stages.iter().rev().find(|&&s| s != 0).copied().unwrap_or(0)` (rightmost
    non-zero, else 0); else `stages.last().copied().unwrap_or(0)` (current
    behavior). Return `Continue(status)`.
  - `Stopped(sig)` case unchanged (job-control stop); `$PIPESTATUS` is set only
    on normal completion (note: bash's stopped-pipeline PIPESTATUS is an edge
    case, out of scope).

### 4. `$PIPESTATUS` at the other leaf sites (`src/executor.rs`)
- **`run_single`** (single simple command — external / builtin / function call /
  assignment): when it produces `ExecOutcome::Continue(status)`, call
  `shell.set_pipestatus(&[status])` before returning. (Function calls resolve to
  `Continue(n)` here, so they correctly get `[n]`, matching bash's opacity. The
  inner pipelines already overwrote PIPESTATUS during the call; the outer
  `[n]` write is the final, correct value.) For non-`Continue` outcomes
  (`Exit`/`FunctionReturn`/`LoopBreak`/`LoopContinue` from `exit`/`return`/`break`/
  `continue`), do not write — they propagate; the resolving site (e.g. the
  function-call → `Continue(n)`) handles the write.
- **Subshell foreground path** (the `Command::Subshell { .. }` arm in
  `run_command`, ~line 161): after the parent obtains the subshell's exit status,
  `shell.set_pipestatus(&[status])` (1-element — the subshell is one forked unit).
- **Compound runners** (`run_if`/`run_while`/`run_for`/`run_case`/`run_select`/
  brace-group) deliberately do NOT call `set_pipestatus` — transparency falls out,
  since their inner leaf commands (`run_single`/`run_multi_stage`) already updated
  it. (Confirm none of them set it indirectly.)

This yields bash's exact model: every leaf simple-pipeline / forked unit writes
`$PIPESTATUS`; compounds are transparent.

## Testing

1. **Unit tests** (`src/builtins.rs` / `src/shell_state.rs`):
   - `option_get`/`option_set` round-trip for `pipefail`; `set -o` listing
     includes `pipefail`.
   - `set_pipestatus` writes a readable `VarValue::Indexed` (e.g. set `[0,1,0]`,
     assert `get_array`/element lookups).
   - pipefail status computation: rightmost-non-zero over a few vectors (extract
     a tiny pure helper if it reads cleanly, else cover via integration).
   - `dollar_dash_value` does NOT include a pipefail letter.
2. **Integration tests** (`tests/pipefail_integration.rs`, binary-driven):
   - `true|false|true` → `${PIPESTATUS[@]}`=`0 1 0`; `${PIPESTATUS[1]}`=`1`;
     `${#PIPESTATUS[@]}`=`3`.
   - pipefail off (default): `false|true; echo $?` → 0. pipefail on:
     `false|true; echo $?` → 1; `(exit 2)|(exit 3); echo $?` → 3; `true|true` → 0.
   - simple command → `(1)` for `false`, `(0)` for `true`.
   - **transparency**: `if false; then :; fi` → `(1)`; `for i in 1; do true|false;
     done` → `(0 1)`; `{ true|false; }` → `(0 1)`; subshell `(true|false)` → `(1)`;
     function `f(){ true|false; }; f` → `(1)`.
   - `set -o | grep pipefail` shows `off`; after `set -o pipefail`, `on`.
3. **bash-diff harness** `tests/scripts/pipefail_diff_check.sh` (huck's 10th):
   all the above (PIPESTATUS arrays after pipelines/simple/compound/subshell/
   function, and pipefail exit codes) byte-identical to bash 5.2.

## Scope / edge notes (document in M-50)
- `! pipeline` negation isn't parsed by huck (pre-existing); the pipefail/`!`
  interaction is therefore out of scope.
- `$PIPESTATUS` for a *stopped* (Ctrl-Z) pipeline is not set (job-control edge);
  out of scope.
- `$PIPESTATUS` is a normal indexed-array var (readable/assignable, overwritten
  each leaf command), matching bash. Minor: writing it allocates a small array
  per simple command (acceptable; bash maintains it similarly).

## File-change map

| File | Change |
|------|--------|
| `src/shell_state.rs` | `ShellOptions.pipefail`; `Shell::set_pipestatus(&[i32])` writing a `VarValue::Indexed` PIPESTATUS var; unit tests |
| `src/builtins.rs` | `SHELL_OPTIONS` pipefail entry (short None); `option_get`/`option_set` arms; unit tests |
| `src/executor.rs` | `wait_pipeline_raw` → `AllExited(Vec<i32>)`; `run_multi_stage` sets PIPESTATUS + pipefail status; `run_single` sets PIPESTATUS on `Continue`; subshell arm sets PIPESTATUS; (compounds untouched) |
| `tests/pipefail_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/pipefail_diff_check.sh` | NEW — huck's 10th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-50 → `[fixed v83]`; changelog; summary stamp; README v83 row |
