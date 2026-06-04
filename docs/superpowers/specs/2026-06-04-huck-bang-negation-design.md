# huck v85 — `!` pipeline negation Design

**Status:** approved design, ready for implementation plan.
**Fixes:** `!` pipeline negation is not parsed by huck — `if ! cmd; then …`, `! cmd`, `while ! cmd`, etc. treat `!` as a command ("command not found: !"). Discovered loading a stock Debian `~/.bashrc` (`if ! shopt -oq posix; then`).
**Branch (impl):** `v85-bang-negation` (created from `main` at plan time).

## Scope

Implement `!` at the **pipeline level** (POSIX `pipeline ::= ['!'] pipe_sequence`),
not only in `if` conditions. One pipeline-level fix naturally covers `if ! cmd`,
`while ! cmd`, `until ! cmd`, standalone `! cmd`, `! a | b`, `! cmd && …`, and `!`
before compound commands (`! { … }`, `! ( … )`, `! if …`). It also wires the
`set -e`/ERR-trap `!`-exemption currently documented as "moot" (M-22, M-50, M-08
notes).

`!` as a *test argument* (`[ ! -e x ]`, `test ! -e x`) is NOT pipeline negation
and is unaffected (it's not at command position). `[[ ! … ]]` is handled by the
existing `[[ ]]` test parser (`parse_test_not`) — a separate path, untouched.

## Verified bash 5.2 semantics

- `! false`→`$?`=0; `! true`→1.
- `! false | true`→`$?`=1, **`PIPESTATUS=(1 0)` (raw, NOT negated)**.
- `set -o pipefail; ! false | true`→0 (negation applies AFTER pipefail).
- `set -e; ! true; echo survived=$?`→prints `survived=1` (a `!`-pipeline is
  **exempt from errexit** even when its result is non-zero).
- `if ! false; then …` runs the branch; `while ! true; do …` doesn't loop.
- `! if true; then false; fi`→0; `! { false; }`→0; `! (exit 3)`→0 (`!` prefixes
  compounds).
- `! false && echo` → runs `echo` (`!` binds to the pipeline, then `&&`).
- `! ! false`→`$?`=1 (double negation — count parity).

## Design

### 1. AST + parser (`src/command.rs`)
- Add `pub negate: bool` to `Pipeline { negate, commands }`. Update all `Pipeline { … }` construction sites (`grep -rn "Pipeline {" src/`) to `negate: false` (the default).
- At **command position** — the top of `parse_command`, BEFORE keyword dispatch (so `! if …` / `! while …` / `! { … }` work) — consume a run of standalone `!` words via the existing `is_bang_word`. Count them; `negate = (count % 2 == 1)`. Then parse the following command via the normal dispatch and attach negation:
  - if the parsed command is `Command::Pipeline(p)` → set `p.negate = negate` (it was `false`);
  - else (a compound `Command::If`/`While`/`For`/`Case`/`Select`/`Subshell`/brace, or any non-Pipeline) → wrap as `Command::Pipeline(Pipeline { negate, commands: vec![that_command] })` (a 1-element pipeline; `run_pipeline` already unwraps a single-element pipeline via `run_command`).
- Only a STANDALONE `!` word triggers this (already what `is_bang_word` checks: a single unquoted Literal `!`). A `!` that is an argument (e.g. `[ ! …`) is consumed as a normal arg by the simple-command parser, not here, because by then we're past command position.
- Edge — bare `!` (followed by a terminator/EOF, no command): match bash, which treats it as negating an empty/true pipeline → exit status 1. Simplest: if no command follows the bang run, produce an empty negated pipeline whose execution yields `Continue(1)` for odd parity (or `Continue(0)` for even). (If this proves awkward, a `ParseError` is an acceptable fallback — confirm against bash at implementation time; bare `!` is pathological.)

### 2. Execution (`src/executor.rs`)
- In `run_pipeline`, compute the result as today (single-stage → `run_command(inner)`; multi-stage → `run_multi_stage`, which already returns the pipefail-aware status and has set raw `$PIPESTATUS`). THEN, if `pipeline.negate`, transform the outcome:
  - `ExecOutcome::Continue(s)` → `Continue(if s == 0 { 1 } else { 0 })`.
  - leave `Exit`/`FunctionReturn`/`LoopBreak`/`LoopContinue` unchanged (they propagate; a `!` doesn't invert control flow).
- `$PIPESTATUS` is written by `run_multi_stage`/`run_single` BEFORE this negation, so it stays raw — matching bash.

### 3. `set -e` / ERR-trap exemption (`src/executor.rs`)
- In `execute_sequence_body`, at the two sites where ERR fires + `maybe_errexit` is consulted on a non-zero `Continue(c)` (the `seq.first` site and the `seq.rest` loop site), add a guard: if the just-run command is a negated pipeline (`matches!(cmd, Command::Pipeline(p) if p.negate)`), SKIP firing ERR and skip `maybe_errexit` — exactly like the existing `next_is_or` and condition-context exemptions. `$?` is still set to the (negated) status. This matches bash, which exempts `!`-pipelines from `set -e` regardless of their result.

## Testing

1. **Parser unit tests** (`src/command.rs`): `! cmd` → `Pipeline{negate:true,[cmd]}`; `! a | b` → negate + 2 stages; `! ! cmd` → `negate:false` (even parity); `! if true; then :; fi` → `Pipeline{negate:true,[Command::If]}`; `[ ! -e x ]` → the `!` is an ARG of `[` (NOT a negated pipeline); `[[ ! -e x ]]` still routes to the `[[ ]]` test parser.
2. **Integration tests** (`tests/bang_negation_integration.rs`): `! false`→0, `! true`→1; `if ! false; then echo y; fi`→`y`; `while ! true; do echo x; done`→(no x); `! false && echo r`→`r`; `! false | true` → `$?`=1 + `${PIPESTATUS[@]}`=`1 0`; `set -e; ! true; echo survived`→`survived` printed; `set -o pipefail; ! false | true; echo $?`→0; `! { false; }`→0; `! (exit 3)`→0; `! ! false; echo $?`→1.
3. **bash-diff harness** `tests/scripts/bang_negation_diff_check.sh` (huck's 12th): the above byte-identical to bash 5.2.

## Out of scope
- `time` pipeline prefix (POSIX groups `!` and `time` at the same position) — separate, not requested.
- `!` history expansion at the lexer level — unrelated; the lexer already yields `!` as a Word here.

## File-change map

| File | Change |
|------|--------|
| `src/command.rs` | `Pipeline.negate`; bang-run detection at the top of `parse_command` (count parity; wrap compounds); update `Pipeline {…}` construction sites; parser unit tests |
| `src/executor.rs` | `run_pipeline` negates the `Continue` status when `pipeline.negate`; `execute_sequence_body` skips ERR/errexit for negated pipelines |
| `tests/bang_negation_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/bang_negation_diff_check.sh` | NEW — huck's 12th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | new `[fixed v85]` entry; update the M-22/M-50/M-08 "`!` is moot/unparsed" notes; changelog; summary stamp; README v85 row |
