# v166: `$()`/backtick-aware `scan_braced_operand` (fix L-52) — Design

**Status:** approved 2026-06-16
**Iteration:** v166
**Origin:** L-52, logged during v165 as the sibling of the resolved L-10. v165
fixed the `${…}` operand *split* scanners; this fixes the operand *body
extraction* (`scan_braced_operand`), which has the same `$()`-blindness.

## Goal

Make `scan_braced_operand` skip command substitutions (`$(…)`, `$((…))`,
backticks) when extracting a `${…}` operand body, so a literal `}` (or `)`)
*inside* a command substitution does not prematurely close the operand.

## Problem (verified against the current binary)

`scan_braced_operand` (`src/lexer.rs:2019`) extracts a `${…}` operand body up to
its matching `}`, tracking `${`-nesting depth and single/double-quote spans. It
does **not** recognize `$(…)` or backtick command substitutions, so an
**unquoted** `}` inside one ends the operand early, leaving an unbalanced `$(` /
`` ` `` that the operand re-parse then rejects:

| Input | bash | huck (current) |
|---|---|---|
| `s=a}b; echo "${s/$(echo a}b)/Z}"` | `Z` | `syntax error: unterminated command substitution` |
| `s=xy; echo "${s/`echo a}b`/Z}"` (backtick) | `xy` | `syntax error` |

A **quoted** `}` inside the command substitution already works today
(`${s/$(echo "}")/Z}` → both shells agree), because `scan_braced_operand`'s
existing double/single-quote arms consume the quoted span — so only the
*unquoted* `}` inside a command substitution diverges. v165's split fix is
unaffected (verified).

## Design

### Add command-substitution skipping to `scan_braced_operand`

Two changes to the scan loop, plus one small helper:

1. **`$(` in the `'$'` arm.** Today the `'$'` arm raises `depth` only for `${`.
   Extend it so that when `$` is followed by `(`, the whole command
   substitution is consumed **verbatim** through its matching `)` (tracking
   nested parens, skipping quoted spans), appended to `body`. This covers
   `$(…)`, `$((…))` (the inner `(` raises paren depth), and `$( (…) )`. Nested
   `$( … $() … )` is handled because the inner `$(` is re-detected and raises
   paren depth.

2. **New backtick arm.** Add a `` Some('`') `` arm that consumes a backtick
   command substitution verbatim through the matching **unescaped** backtick
   (honoring `\` escapes inside, per bash's backtick rules), appended to `body`
   — structurally like the existing single-quote arm.

3. **Helper** `consume_paren_cmdsub_verbatim(chars, out) -> Result<(),
   LexError>`: given the cursor positioned just after a `$(` (the `$(` already
   appended to `out`), consume through the matching `)` — incrementing on `(`,
   decrementing on `)`, returning when depth hits zero; skip `'…'` and `"…"`
   spans (double-quote honors `\`); treat `\x` as two verbatim chars; a nested
   `$(` (a `$` followed by `(`) raises depth. Runs out of input → `Err(LexError::
   UnterminatedBrace)` (consistent with the function's existing error for an
   unterminated operand). Mirrors `scan_paren_substitution`'s loop but appends
   text instead of parsing.

The `depth` (brace) counter and the `${`-nesting case are unchanged; the only
new behavior is that text inside a `$(…)` / backtick is now passed through
verbatim instead of being scanned for `}`.

### Why this is correct and narrow

The change is purely additive: it intercepts `$(`/backtick *before* their inner
chars reach the brace/`}` logic. For any operand with no command substitution,
the new arms never fire and behavior is identical. A `}` outside a command
substitution still closes the operand; a quoted `}` inside still works (now via
the verbatim skip rather than the quote arm, same result). The pre-existing
"`${…}` nested inside a double-quoted span" known-limitation (documented in the
function's header comment) is unrelated and unchanged.

## Scope boundary (v164/v165 lessons)

Narrow: only `scan_braced_operand` + the new helper. This iteration does **not**
refactor v165's `split_modifier_operand` to share the helper (a tempting DRY
move — both now do `$()`-skipping — but out of scope and adds regression
surface; noted as a future cleanup). It does **not** handle `<(…)`/`>(…)`
process substitution inside an operand (exotic; not part of L-52). `arith.rs`
(L-24) and general `CharCursor` quote helpers remain separate follow-ons.

## Verification

### Correctness

- **Extend** the existing `tests/scripts/param_cmdsub_split_diff_check.sh`
  harness with L-52 fragments asserted byte-identical to bash: an unquoted `}`
  inside `$(…)`, inside backticks, inside `$((…))`, a nested `$( … $() … )` with
  a `}`, and a quoted-`}`-inside-`$()` case (already worked — guards no
  regression). (Keeping these with the v165 cases co-locates all
  command-substitution-in-operand checks.)
- **Unit tests** in `src/lexer.rs` (mirroring the existing
  `scan_braced_operand_*` tests at lexer.rs:5112+): a `$(…)` with an unquoted
  `}` returns the full body (`CharCursor::new("$(echo a}b)/Z}")` →
  `"$(echo a}b)/Z"`); a backtick body with `}`; a nested `$( $() )` with `}`; and
  an unterminated `$(` → `Err(LexError::UnterminatedBrace)`.
- **Full regression:** the whole unit suite, all integration tests, and all 92
  existing bash-diff harnesses stay green; `cargo clippy --lib --bins` clean.
  The existing parameter-expansion tests/harnesses guard that no-command-
  substitution operands are byte-identical before and after.

### Behavior preservation

The refactor is behavior-preserving for every operand that contains no command
substitution (the new `$(`/backtick paths never execute). The L-52 cases are the
only intended change (`syntax error` → bash-matching output).

## Docs / iteration close-out

- On merge, **delete** the L-52 entry from `docs/bash-divergences.md` (resolved)
  and decrement the Tier-4 count 42 → 41.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`.
