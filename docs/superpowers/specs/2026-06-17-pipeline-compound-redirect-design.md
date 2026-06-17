# v176: redirect on a compound-command pipeline stage — Design

**Status:** approved 2026-06-17
**Iteration:** v176
**Origin:** The dominant root cause of the parse-compat sweep's "unexpected token
after command" cluster (~18 of 20 entries): the entire kernel `syscall*` family
(`grep … | sort | tail -1 | ( … ) > "$out"`), plus lvmdump, setupcon, zgrep,
xzgrep, nvm, pktgen/functions.sh. All use a redirect on a compound-command
pipeline stage.

## Problem

`parse_next_stage` (`src/command.rs:2270`) parses each pipeline stage AFTER the
first. For a compound stage it returns the command directly and never parses a
trailing redirect:

```rust
Some(Keyword::Case) => Ok((Command::Case(Box::new(parse_case(iter)?)), false)),
Some(Keyword::LBrace) => Ok((Command::BraceGroup(Box::new(parse_brace_group(iter)?)), false)),
…
// bare `(` subshell:
return Ok((parse_subshell(iter)?, false));
```

Its sibling `parse_command_inner` (the standalone / first-stage path) wraps EVERY
compound with `maybe_wrap_redirects(cmd, iter)`, which consumes any trailing
redirects and produces `Command::Redirected { inner, redirects }`:

```rust
Some(Keyword::Case) => maybe_wrap_redirects(Command::Case(Box::new(parse_case(iter)?)), iter),
Some(Keyword::LBrace) => maybe_wrap_redirects(Command::BraceGroup(Box::new(parse_brace_group(iter)?)), iter),
…
```

So a trailing redirect after a compound stage of a pipeline is never consumed; the
leftover `>`/`2>&1`/… token then fails to parse with "unexpected token after
command", and (because the parser reports the start of the mis-parsed command and
keeps going) cascades into the later `if`/`case`/`fi` "unexpected token" derail
errors seen across the cluster.

Confirmed (huck FAIL, bash OK):
```
echo a | ( cat ) > /tmp/o          echo a | { cat; } > /tmp/o
echo a | ( cat ) 2>&1              ( echo a ) | { cat; } > /tmp/o
x=$( echo a | { cat; } > /dev/null )    # same bug nested in $( … )
```
Works (so the bug is specific): compound stage with NO redirect
(`echo a | (cat)`), any position; and a redirect on a *plain* stage
(`echo a | cat > f`).

## Goal

Let a redirect attach to a compound-command pipeline stage, matching bash, by
mirroring `parse_command_inner` in `parse_next_stage`.

## Design

In `parse_next_stage`, wrap each compound-command arm with
`maybe_wrap_redirects(…, iter)?` before returning `(cmd, false)`, exactly as
`parse_command_inner` does. The arms to wrap:

- arith block (`((…))` stage) — currently `Ok((Command::Arith(body), false))`
- `If`, `While`/`Until`, `For`, `Select`, `Case`
- `LBrace` (brace group), `DoubleBracketOpen` (`[[ … ]]`)
- the bare-`(` subshell (`return Ok((parse_subshell(iter)?, false))`)

Each becomes, e.g.:
```rust
Some(Keyword::Case) => Ok((
    maybe_wrap_redirects(Command::Case(Box::new(parse_case(iter)?)), iter)?,
    false,
)),
…
// subshell:
return Ok((maybe_wrap_redirects(parse_subshell(iter)?, iter)?, false));
```

Unchanged: the function-def arm (`name()` / `function`) — function definitions
take no trailing redirect, and `parse_command_inner` doesn't wrap them either; the
simple-stage arms (`parse_simple_stage` parses its own redirects); and the
`coproc`-as-stage error. The returned `false` (this stage did not consume a `|`)
is still correct — `maybe_wrap_redirects` consumes only redirects, after which the
caller (`parse_pipeline_with_first`, line ~2356) checks for the next `|`.

### Why this is parse-only (no executor change)

`maybe_wrap_redirects` yields `Command::Redirected { inner: <compound>, redirects
}`. `classify_stage` (`executor.rs:5880`) routes anything that isn't a bare
external `Simple(Exec)` to `StageKind::InProcess`, so a `Redirected`-wrapped
compound stage is forked via `fork_and_run_in_subshell`, whose child runs the
command through `run_command`; `run_command`'s `Command::Redirected` arm
(`executor.rs:527`) applies the redirects via the redirect-scope machinery —
inside the forked stage, AFTER the pipe fd was dup'd onto fd 1 — so the explicit
`> file` correctly overrides the pipe. The standalone `( … ) > f` already
exercises this exact path; the fix merely lets the parser build the same AST for a
pipeline stage.

### Behavior

- `echo a | ( cat ) > f`, `… | { … } > "$out"`, `( … ) | { … } > f`, a `2>&1`
  on a compound stage, and the same nested in `$( … )` → parse and execute like
  bash (the compound stage's output goes to the file, not the pipe sink).
- Unchanged: compound stages without a redirect; redirects on plain stages; the
  standalone `( … ) > f`.

## Verification

- **New bash-diff harness** `tests/scripts/pipe_compound_redirect_diff_check.sh`
  — EXECUTING, byte-identical bash↔huck: `echo a | (cat) > F; cat F`,
  `printf 'x\n' | { cat; } > F; cat F`, `(echo x) | { cat; } > F; cat F`, a `2>&1`
  on a compound stage, the redirect nested in `$( … )`, a syscall-style
  `printf '1 a\n2 b\n' | tail -n1 | ( read n x; echo "$n=$x" ) > F; cat F`, and
  `if`/`while`/`case` compound stages carrying a redirect; plus a regression case
  with a compound stage and NO redirect. (Use a per-run temp file under the
  harness's control; assert the file contents.)
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh tools/scripts.tsv` and
  confirm the "unexpected token after command" `HUCK_GAP` cluster drops from ~18
  to ~1 (the `zdiff` outlier) and the total `HUCK_GAP` falls accordingly
  (≈49 → ≈32), with no new `HUCK_LENIENT`/`HUCK_CRASH`.
- Full `cargo test` (0 failures) — including existing pipeline / subshell tests —
  all `tests/scripts/*_diff_check.sh` harnesses green, clippy clean.

## Scope boundary

In scope: wrapping the compound arms of `parse_next_stage` with
`maybe_wrap_redirects`; the new harness; the parse-sweep confirmation. **Not** in
scope: refactoring the `parse_next_stage` / `parse_command_inner` duplication into
one shared dispatcher (a worthwhile but riskier change — left as a follow-on);
function-def-stage redirects; the `zdiff` outlier (a separate `$( … )`-nested
construct, its own future bisect); any executor change (execution already honors
the redirect). No `bash-divergences.md` change (never a tracked divergence).
Record in `project_huck_iterations.md` + `MEMORY.md`.
