# v235: extquote `$'…'`-name gated on double-quote context (M-156) — Design

**Status:** approved (brainstorm 2026-06-28)
**Closes:** M-156.

## Goal

bash decodes a `$'…'` (ANSI-C) quote used as the parameter NAME inside `${…}`
(the `extquote` feature) ONLY when the `${…}` is within DOUBLE QUOTES. huck (since
v234) decodes it unconditionally, so an UNQUOTED `${$'x1'}` expands in huck but is a
`bad substitution` (rc 1) in bash. v235 gates the decode on double-quote context so
huck matches bash in every context: top-level, `:-`/`:=`/`:+` defaults, and `#`/`%`/`/`/`^`/`,`
pattern operands (including nesting).

## Background

### Measured bash 5.2.21 ground truth

| input | bash |
|---|---|
| `x1=V; echo ${$'x1'}` (unquoted) | `${'x1'}: bad substitution` (rc 1) |
| `x1=V; echo "${$'x1'}"` (quoted) | `V` |
| `x1=V; y=${$'x1'}; echo "$y"` | `bad substitution` (rc 1) |
| `x1=V; echo ${x1}${$'x1'}` (unquoted concat) | `bad substitution` (rc 1) |
| `x=notOK; x1=not; echo "${x#${$'x1'%$'t'}}"` | `tOK` |
| `x=notOK; x1=not; echo ${x#${$'x1'%$'t'}}` (unquoted) | `bad substitution` (rc 1) |
| `x1=hi; unset z; echo "${z:-${$'x1'}}"` | `hi` |
| `x1=hi; unset z; echo ${z:-${$'x1'}}` (unquoted) | `bad substitution` (rc 1) |

Rule: `$'…'`-as-name decodes iff the `${…}` is in double-quote context.

### Why v234's naive gate failed

v234 tried gating the decode on the existing `quoted` lexer flag. It worked for
top-level `${$'x1'}` but broke the QUOTED nested pattern case
`"${x#${$'x1'%$'t'}}"` (expected `tOK`, got `bad substitution`). Root cause: the
`#`/`%`/`/`/`^`/`,` PATTERN operands re-parse their body via
`parse_braced_operand_opts(body, enclosing_dquote=false, opts)` — the
`enclosing_dquote` flag is forced `false` ON PURPOSE because it ALSO controls
glob-literalness, and a pattern's glob must stay active even inside double quotes
(`"${x#$p}"` with `p="a?"` → `[b]`, verified — the `?` from the variable stays an
active glob). So the inner `${$'x1'}` of a pattern always parses with `quoted=false`,
and gating on `quoted` wrongly suppressed the decode in the quoted case.

The `:-`/`:=`/`:+` DEFAULT operands do NOT have this problem: `modifier_with_operand`
threads the real `quoted` as `enclosing_dquote` (a default value's quoting genuinely
controls splitting, no glob conflation), so their inner `quoted` flag is correct.

So `enclosing_dquote` conflates two concerns (glob-literalness vs double-quote
context) and cannot be reused for extquote gating in patterns.

## Architecture

All changes are in `crates/huck-syntax/src/lexer.rs`. No engine change. No new
`ParamModifier`/`WordPart` variant.

Add a **new, single-purpose** field `in_dquote: bool` to `LexerOptions`, read ONLY
by the extquote-name gate. Because nothing else reads it, glob-literalness,
word-splitting, single-quote handling, and reconstruction are provably unchanged.

### Change 1 — `LexerOptions`

```rust
pub struct LexerOptions {
    pub extglob: bool,
    /// True when the `${…}` currently being scanned is inside double quotes.
    /// Read ONLY by the extquote `$'…'`-name gate (M-156); does not affect
    /// glob/splitting/quoting of operands.
    pub in_dquote: bool,
}
```

Add a builder:

```rust
impl LexerOptions {
    fn with_in_dquote(self, b: bool) -> Self {
        LexerOptions { in_dquote: b, ..self }
    }
}
```

All existing constructors of `LexerOptions` must initialize `in_dquote: false`
(the build will flag every site; default is `false`).

### Change 2 — the extquote gate

In `scan_braced_param_expansion`'s regular-name path (the `NameScan::Name { name,
decoded }` arm added in v234), the decoded-name guard becomes:

```rust
NameScan::Name { name, decoded } => {
    // A `$'…'`-decoded name is only valid in double-quote context (bash
    // extquote). Unquoted -> runtime bad substitution.
    if decoded && !(quoted || opts.in_dquote) {
        return recover_bad_subst(chars, parts, quoted, dollar_start);
    }
    // (existing) an invalid decoded name is also a bad substitution.
    if decoded && !is_valid_param_name(&name) {
        return recover_bad_subst(chars, parts, quoted, dollar_start);
    }
    name
}
```

`quoted` handles top-level + default operands; `opts.in_dquote` handles pattern
operands. (Order: the `!(quoted || in_dquote)` check first, so an unquoted invalid
name still bad-substs either way.)

### Change 3 — set `in_dquote` at the pattern-operand dispatch arms

In `dispatch_braced_modifier`, the five PATTERN-operand arms pass
`opts.with_in_dquote(quoted || opts.in_dquote)` as the `opts` argument to their
operand scanner (the glob-controlling `enclosing_dquote`/`false` is UNCHANGED):

- `#`/`%` (RemovePrefix/RemoveSuffix, ~3701/3708):
  `modifier_with_operand(chars, false, opts.with_in_dquote(quoted || opts.in_dquote), |w| …)`
- `/` (Substitute, ~3720): `scan_substitution_operand(chars, opts.with_in_dquote(quoted || opts.in_dquote))`
- `^`/`,` (Case, ~3733/3746): `scan_optional_braced_operand(chars, opts.with_in_dquote(quoted || opts.in_dquote))`

`opts` already flows `parse_braced_operand_opts → scan_dollar_expansion →
scan_braced_param_expansion`, so the inner expansion's gate sees the propagated
`in_dquote`. No function-signature changes.

The `quoted || opts.in_dquote` formula makes it compose through nesting: a pattern
inside a pattern (`"${x#${y#${$'x1'}}}"`) carries `in_dquote=true` inward even
though each inner `quoted` param is `false`.

The `:-`/`:=`/`:+` default arms are NOT changed — `modifier_with_operand` already
threads `quoted`, so the gate's `quoted` term handles them (a default inside a
quoted context is parsed with `enclosing_dquote=quoted`, so a nested pattern within
it sees the right `quoted` to seed `in_dquote`).

## Edge cases & interactions

- **Composition:** verified by the formula — defaults-within-patterns, patterns-within-defaults,
  and patterns-within-patterns all propagate `in_dquote` correctly because each pattern arm
  ORs the current `quoted` with the inherited `opts.in_dquote`.
- **`$"…"` (locale) name:** unchanged — already routes to `recover_bad_subst` in all
  contexts (bash bad-substs it regardless of quoting).
- **Invalid decoded name (`${$'x\ty'}`):** unchanged — still `recover_bad_subst` (the
  `!(quoted || in_dquote)` check runs first for the unquoted case; the
  `!is_valid_param_name` check covers the quoted case).
- **Glob / word-splitting / single-quote / set -x reconstruction:** PROVABLY unchanged —
  `in_dquote` is read only by the extquote gate; the glob-controlling `enclosing_dquote`
  argument to every operand scanner is untouched.
- **Substring offset/length operands** (`${x:off:len}`, `parse_braced_operand_opts(…, false, …)`
  at ~3977/3979): out of scope — arithmetic context, extquote-name does not apply; left as-is.

## Out of scope

- Disentangling `enclosing_dquote` itself (the glob-vs-dquote conflation) beyond what the
  extquote gate needs — not required and higher-risk.
- `extquote` as a real `shopt` (huck has no such option; always-on-when-quoted matches default bash).

## Testing

- **Lexer unit tests** (`lexer.rs` `mod tests`): `tokenize("${$'x1'}")` (unquoted, the
  default `tokenize` path) → the part is `BadSubst`; a helper that tokenizes within a
  double-quoted context → decoded `Var`/`ParamExpansion` name `x1`. (Use whatever existing
  test helper produces a quoted `${…}`; if none, assert the unquoted→BadSubst direction in
  the lexer and cover the quoted/nested directions in integration tests below.)
- **Integration tests** (extend `tests/param_indirect_extquote_integration.rs`): for each
  row of the ground-truth table — unquoted top-level/default/pattern → rc 1 + stderr
  "bad substitution"; quoted top-level/default/nested-pattern → the value (`V`/`hi`/`tOK`),
  rc 0. Include `extquote_name_unquoted_is_bad_subst` (already added in v234) and confirm it
  still passes, plus new `extquote_pattern_unquoted_is_bad_subst` and
  `extquote_pattern_quoted_decodes`.
- **Diff harness:** extend `tests/scripts/param_indirect_extquote_diff_check.sh` (or a new
  `param_extquote_qctx_diff_check.sh`) with the full ground-truth table — byte-identical
  bash↔huck (the `checkf_badsubst` form for the bad-subst rows, plain `checkf` for the value
  rows).
- **Regression:** the v234 `extquote_nested_pattern_operand` test (`"${x#${$'x1'%$'t'}}"` →
  `tOK`) MUST stay green — it is the proof the gate didn't over-fire.
- **Full `cargo test --workspace`** green; build warning-clean.
- **Docs (post-merge):** DELETE M-156 from `docs/bash-divergences.md`. Note the two MINOR
  display residuals from M-156 (error-message name-form for `${$'x\ty'}` / `${$"x1"}`) are
  NOT closed by v235 — re-home them as a small `[deferred]` entry if they should stay tracked.
