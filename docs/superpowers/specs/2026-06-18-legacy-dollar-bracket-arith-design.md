# v188: legacy `$[ expr ]` arithmetic expansion — Design

**Status:** approved 2026-06-18
**Iteration:** v188
**Origin:** Parse sweep gap on `samples/pktgen/functions.sh` (×2: kernel-headers
6.8.0-110 and -124). Reported as `line 209: syntax error: unexpected token after
command`, but that label is misleading — bisection showed the real trigger is
`local max=$[ 2**(len*2)-1 ]` in the `validate_addr` body (the error is
mis-attributed to the function's start line). The deeper cause: **huck does not
implement `$[ … ]` (bash's deprecated arithmetic-expansion form) at all.**

## Current (broken) behaviour

`scan_dollar_expansion` (`src/lexer.rs`) has no `$[` arm, so `$[…]` is lexed as a
literal `$` followed by ordinary shell tokens:

- `echo $[3+4]` → prints `$[3+4]` literally (bash: `7`).
- `echo $[ 2 * 5 ]` → the `*` is **glob-expanded** to filenames (bash: `10`).
- `echo $[ 2**(len*2)-1 ]` → the `(` becomes a stray `LParen` token mid-command →
  `syntax error: unexpected token after command` (the visible parse gap).

Simple forms (`$[8-x]`, `$[16#5]`) happen to survive lexing as a plain word but
still expand wrong.

## bash contract (verified)

`$[ expr ]` is exactly equivalent to `$(( expr ))` — same arithmetic grammar and
evaluation. Verified against bash 5.x:

| fragment | bash |
|---|---|
| `echo $[3+4]` | `7` |
| `echo $[2**(3*2)]` | `64` |
| `a=(5 6); echo $[a[1]+1]` | `7` (array subscript) |
| `echo $[$(echo 3)+1]` | `4` (nested command sub) |
| `x=4; echo $[${x}+1]` | `5` (nested `${…}`) |
| `echo "$[1+2]"` | `3` (inside double quotes) |
| `echo $[1?8:9]` | `8` (ternary) |
| `echo $[ $[2+3] * 2 ]` | `10` (nested `$[…]`) |
| `echo $[1,2,3]` | `3` (comma) |
| `echo $[10/3]` | `3` |

An arithmetic body can therefore contain brackets (array subscripts `a[1]`,
`${a[i]}`) and nested expansions whose own `]` must NOT close the `$[…]`.

## Goal

Implement `$[ expr ]` as a lex-time alias of `$(( expr ))`, emitting the existing
`WordPart::Arith`. All arithmetic semantics (`**`, ternary, `a[i]`, `16#`, comma,
nested expansions) come for free by reusing `arith_string_to_word` and the
`arith.rs` evaluator. **No AST, parser, expand, or executor change.**

## Design

### 1. New `$[` arm in `scan_dollar_expansion` (`src/lexer.rs`)

Alongside the existing `Some('(')` and `Some('{')` arms (`src/lexer.rs:1762`):

```rust
        Some('[') => {
            chars.next(); // consume '['
            let inner = scan_legacy_arith_body(chars)?;
            let body = arith_string_to_word(&inner, opts)?;
            parts.push(WordPart::Arith { body, quoted });
        }
```

This mirrors the `$((` arm: same `WordPart::Arith`, same `quoted` flag, same
`opts` (so extglob inheritance into nested command subs behaves identically — the
v169 path). It is reached wherever a `$` expansion is scanned, including inside
double quotes (`quoted == true`) and inside other expansions.

There is no ambiguity with the current `_`-arm literal handling: bash always
treats `$[` as arithmetic, and `$[…]` is currently broken in huck, so adopting
bash's behaviour is strictly a fix.

### 2. New close-finder `scan_legacy_arith_body` (`src/lexer.rs`)

Modeled on `scan_arith_body` but terminated by the matching `]` and **fully
aware** (per the approved design): it collects all text verbatim into the body
string while finding the terminating `]`, so a stray `]`/`[` inside a quoted span
or nested command sub cannot miscount.

State and rules (the opening `$[` is already consumed; this scans the body):

- `depth: usize` — raw bracket nesting. A raw `[` → `depth += 1`; a raw `]` with
  `depth > 0` → `depth -= 1`; a raw `]` with `depth == 0` → **done** (return the
  collected body, the closing `]` consumed but not appended). This balances array
  subscripts (`a[1]`), `${a[i]}`, and nested `$[…]` automatically (their brackets
  pair up as raw brackets).
- `'…'` — consume verbatim through the closing `'` (single quotes: no escapes).
- `"…"` — consume verbatim through the closing `"`, honoring `\` (so `\"` does not
  close the span). Brackets inside are not counted.
- `$` followed by `(` — reuse `scan_cmdsub_body` to skip the command sub verbatim
  (it is itself quote/nest-aware and handles `$( … )`, `$(( … ))`, `$( ( … ) )`).
  Append `$(` + the body + the closing `)` so the text stays verbatim for
  `arith_string_to_word`.
- `$` followed by `{` — brace-balanced verbatim skip through the matching `}`
  (tracking `{`/`}` depth, honoring `'…'`/`"…"` inside so a `}` in a quote does
  not close early), so a `]` inside `${a[i]}` / `${x:-]}` is not counted.
- `\` — append it and the next char verbatim (a backslash escape; the next char is
  not interpreted as a bracket/quote).
- any other char — append verbatim.
- EOF before the closing `]` → `Err(LexError::UnterminatedLegacyArith)` (a new
  variant, mirroring `UnterminatedArith`).

Notes:
- Paren depth is intentionally NOT tracked: the terminator is `]`, and `(`/`)` do
  not contain it; parens pass through verbatim into the body (so `2**(len*2)`
  reaches `arith_string_to_word` intact).
- A `$` not followed by `(` or `{` (e.g. `$x`, `$1`, `$[…]` nested) is appended as
  an ordinary char; a nested `$[…]`'s brackets balance via the raw-bracket depth,
  so no special `$[` recursion is needed.

### 3. New `LexError::UnterminatedLegacyArith`

Add the variant next to `UnterminatedArith` (`src/lexer.rs:~22`). Wire it into:
- the lexer error → user-facing message in `src/shell.rs` (mirror the
  `UnterminatedArith` message, e.g. `unterminated '$['  (expected ']')`).
- `continuation.rs` `is_unterminated_lex` (or equivalent): an unterminated `$[`
  across lines must request REPL continuation, exactly like `$((`. (The v183
  lesson: every new unterminated lex variant must be registered in the
  continuation classifier, or multi-line input via stdin breaks.)

## Verification

- **New bash-diff harness** `tests/scripts/legacy_arith_diff_check.sh` (executing,
  byte-identical stdout+exit). Cases: the real pktgen shapes
  (`echo $[ 2**(len*2)-1 ]` with `len=4`, `echo $[ IP6 ? 128 : 32 ]`,
  `d=ff; echo $[ 16#$d ]`); array subscript `a=(5 6); echo $[a[1]+1]`; a `]`
  inside `${…}` `a=(9); echo $[ ${a[0]} + 1 ]` (proves the `]`-in-`${}` is
  skipped, with a real numeric result `10`); nested command sub
  `echo $[$(echo 3)+1]`; nested `$[…]` `echo $[ $[2+3] * 2 ]`; inside double
  quotes `echo "$[1+2]"`; ternary, comma, division; and a control where `$[`
  does NOT appear (plain `echo $((1+1))` unchanged). (The "aware" cases where a
  `]` sits inside a quoted span or a command sub — `$[ $(echo ']')+1 ]`,
  `$[ "x]" ? 1 : 2 ]` — are exercised in the lexer unit tests below via
  body-capture assertions rather than the harness, since a `]` produced by such a
  sub makes the arithmetic invalid in BOTH shells; the unit test cleanly proves
  the close-finder did not close early.)
- **Lexer unit tests** (`src/lexer.rs` `mod tests`, near the existing
  `scan_arith_*` tests):
  - `$[2**(3*2)]` lexes to a single `Word` whose part is `WordPart::Arith` with
    body text `2**(3*2)` (assert the body string, before evaluation).
  - the aware cases stop at the correct `]`: `$[ $(echo ']')+1 ]` and
    `$[ "x]" ? 1 : 2 ]` produce an `Arith` part (not a premature close); assert
    the captured body includes the full inner text.
  - `$[a[1]+1]` → `Arith` body `a[1]+1` (raw-bracket balancing).
  - unterminated `$[ 1+2` → `Err(LexError::UnterminatedLegacyArith)`.
- **Continuation test**: feeding `echo $[1 +` then `2]` over two stdin lines
  produces `3` (the classifier requests continuation rather than erroring). A
  small assertion in the continuation tests (or a stdin-driven integration test)
  matching the existing `$((` continuation coverage.
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh`; confirm both
  `samples/pktgen/functions.sh` copies parse (`huck -n` rc 0). Report `HUCK_GAP`
  from the 5 baseline (expect 5→3); `HUCK_LENIENT`/`HUCK_CRASH`/`HUCK_TIMEOUT`
  stay 0. byobu-ulevel (`\`⏎`(` array) and perf-completion (`${=1}`) remain.
- **Full `cargo test`** (0 failures). UP-FRONT grep `tests/` + `src/` for any test
  asserting the OLD `$[…]`-as-literal behaviour (none expected — the form was
  broken/literal, not deliberately tested); update only genuine old-behaviour
  tests if found.
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

Sweep-found; no tracked `M-*`/`L-*` divergence covers `$[…]`, so **no
`bash-divergences.md` change**. Record the iteration in
`project_huck_iterations.md` + `MEMORY.md`; update the backlog note (pktgen ×2
cleared; remaining sweep gaps = byobu-ulevel `\`⏎`(` array literal and
perf-completion `${=1}` parse-time strictness).

## Scope boundary

In scope: the `$[` arm in `scan_dollar_expansion`, the `scan_legacy_arith_body`
close-finder, the `UnterminatedLegacyArith` variant + its message + continuation
wiring, the new harness + lexer/continuation tests. **Not** in scope: the byobu
`\`⏎`(` array bug (separate) and the perf-completion `${=1}` strictness gap
(separate, arguably intentional); any change to `$((…))`, `arith.rs`, the AST, or
the executor (none needed — the body reuses the existing arithmetic path).
