# v187: function-definition trailing redirects (resolves M-09b) — Design

**Status:** approved 2026-06-18
**Iteration:** v187
**Origin:** Tracked divergence **M-09b** ("Definition-attached redirections"),
surfaced by the parse sweep on `create-repo.sh` (`function cleanup { … } >&2`).
bash attaches a trailing redirect to a function DEFINITION; it is applied at
every call. huck rejects both forms (`name() body >file` and `function name body
>file`) with `syntax error: function definition: expected '()' and a
compound-command body`.

## bash contract (verified)

The redirect is part of the function and is applied (with filename expansion) at
EACH call, not at definition time:

- `f() { echo "[$1]"; } >/tmp/fo; f A; f B` → `/tmp/fo` = `[B]` (each call
  re-opens `>` and truncates; the last call wins).
- `g() { echo line; } >>/tmp/fa; g; g` → `/tmp/fa` = `line`⏎`line` (append).
- `D=/tmp/fa; h() { echo hi; } >"$D"; D=/tmp/fb; h` → writes to `/tmp/fb` — the
  redirect filename is expanded at CALL time (D's value when `h` runs), not at
  definition.
- Both forms (`name() …` and `function name …`) behave identically.
- `f() { echo hi; } >&2; declare -f f` prints the body with the redirect
  (bash: `} 1>&2`).

## Root cause

A function body is parsed by `parse_command`, which — for a compound command
followed by redirects (`{ … } >f`) — already returns
`Command::Redirected { inner: <compound>, redirects: [>f] }`
(via `maybe_wrap_redirects`). But `is_function_body_shape` (`src/command.rs`),
which both `parse_function_def` (the `name()` form) and
`parse_function_keyword_def` (the `function` form) call to validate the body,
accepts only the compound shapes:

```rust
fn is_function_body_shape(body: &Command) -> bool {
    matches!(body,
        Command::If(_) | Command::While(_) | Command::For(_) | Command::Select(_)
        | Command::Case(_) | Command::BraceGroup(_) | Command::Subshell { .. }
        | Command::DoubleBracket { .. } | Command::Arith(_) | Command::ArithFor(_))
}
```

It does NOT accept `Command::Redirected`, so a redirected body is rejected.

## Goal

Accept a `Redirected` body whose inner is a valid function-body shape, storing it
as the function body. The executor already runs a `Redirected` body by applying
its redirects around the inner with call-time filename expansion — so the
correct bash semantics fall out with no executor/AST change. (Validated by an
end-to-end spike of exactly this change: define+call, each-call truncation,
`>>`, `>&2`, and call-time filename expansion all matched bash.)

## Design

One recursive guard at the top of `is_function_body_shape`:

```rust
fn is_function_body_shape(body: &Command) -> bool {
    // A redirected compound (`{ … } >file`) is a valid function body — the
    // redirect attaches to the definition and is applied (with call-time
    // expansion) on every call. The Redirected body is stored and re-executed
    // per call, which gives bash's semantics with no executor change (M-09b).
    if let Command::Redirected { inner, .. } = body {
        return is_function_body_shape(inner);
    }
    matches!(body,
        Command::If(_) | Command::While(_) | Command::For(_) | Command::Select(_)
        | Command::Case(_) | Command::BraceGroup(_) | Command::Subshell { .. }
        | Command::DoubleBracket { .. } | Command::Arith(_) | Command::ArithFor(_))
}
```

### Why this is correct and minimal

- `parse_command` already produces the `Redirected` wrapper for `compound >f`, so
  no parser change is needed; both definition forms route through
  `is_function_body_shape`, so both are fixed by this one change.
- The function table stores `Box<Command>` (the body AST) and re-executes it on
  each call; a `Redirected` body therefore re-applies its redirects every call,
  with the filename expanded at call time — matching bash.
- A `Redirected` wrapping a NON-compound inner (e.g. a simple command, `f()
  echo hi >f`) is still rejected (recursion bottoms out at a non-shape) — bash
  also requires a compound function body.
- `declare -f` reconstructs via `generate::function_to_source`, which already
  renders `Command::Redirected` (`src/generate.rs:71`) — so a redirected
  function round-trips (exact wording is huck's normalized format, not byte-
  identical to bash, consistent with the existing `declare -f` behavior).

### Behavior after the fix

- Both forms with `>file` / `>>file` / `>&2` parse, define, and apply the
  redirect at each call with call-time expansion (the contract above).
- `create-repo.sh`: `huck -n` silent, rc 0.
- Plain functions (no trailing redirect) unchanged.

## Verification

- **New bash-diff harness** `tests/scripts/func_redirect_diff_check.sh`
  (executing, byte-identical stdout+exit): define+call with `>file` then `cat`
  the file (each-call truncation, last wins); `>>file` append over two calls;
  `>&2` dup with the call capturing `2>file`; call-time filename expansion (`D`
  changed between def and call); the `function name …` keyword form; and a
  control function with NO redirect. (Cases that write files `cat` them back so
  stdout carries the result.)
- **Parser unit test** (`src/command.rs` `mod tests`): `function f { :; } >&2`
  and `f() { :; } >&2` each parse to a `Command::FunctionDef` whose `body` is a
  `Command::Redirected` wrapping a `BraceGroup`; `f() echo hi` style
  (non-compound) still errors `FunctionBody`.
- **`declare -f` round-trip**: `f() { echo hi; } >&2; declare -f f` renders the
  redirect, and feeding that output back defines an equivalent function (a
  diff-check or integration assertion that the rendered text contains the
  redirect and re-parses).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh`; confirm `create-repo.sh`
  parses (`huck -n` rc 0). Report `HUCK_GAP` from the 6 baseline;
  `HUCK_LENIENT`/`HUCK_CRASH`/`HUCK_TIMEOUT` stay 0. (byobu-ulevel remains a gap —
  the separate `\<NL>`-array bug, v188.)
- **Full `cargo test`** (0 failures). UP-FRONT grep `tests/` + `src/` for
  function-definition / `is_function_body_shape` tests; the existing function
  parse + `declare -f` tests must stay green (plain function bodies unchanged).
  Update only genuine old-behavior tests (none expected — the change only adds
  acceptance of a previously-rejected shape).
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

**Resolves M-09b:** delete the M-09b entry from `docs/bash-divergences.md` and
decrement the Tier-2 count by 1. Record the iteration in
`project_huck_iterations.md` + `MEMORY.md`; update the backlog note (the
function-definition cluster's create-repo.sh side is fixed; byobu-ulevel's
`\<NL>`-array bug remains for v188).

## Scope boundary

In scope: the `Redirected`-body acceptance in `is_function_body_shape`, the new
harness + parser/declare-f tests, the M-09b deletion. **Not** in scope: the
`\<NL>`-before-array-`(` bug (byobu-ulevel — v188); the relaxed function-name
charset M-09a (still `[deferred]`); any AST/executor change (none needed). No
behavior change to plain function definitions.
