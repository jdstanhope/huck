# v167: unify the `$()`-scan loops onto one kernel — Design

**Status:** approved 2026-06-16
**Iteration:** v167
**Origin:** the v166 close-out flagged three near-identical `$(…)` paren/quote
scanning loops. This is the DRY follow-on. It is the same class of duplication
that *caused* L-10 (the `${…}` split scanners had drifted from
`scan_paren_substitution`'s `$(`-awareness); collapsing the loops to one source
of truth prevents that drift from recurring.

## Goal

Extract the per-character `$(…)` command-substitution body scan (nested
paren depth + quote spans + escape) into one private kernel, and route the
three current copies through it — with no change in observable behavior.

## Problem

Three functions in `src/lexer.rs` each implement "scan a `$(…)` body, tracking
nested parens, skipping `'…'`/`"…"` spans (double honoring `\`) and `\`-escapes":

1. **`scan_paren_substitution`** — the real `$(…)` tokenizer (hot path). Collects
   the body (excluding the closing `)`), then **parses** it to a `Sequence` via
   `parse_substitution_body`. Errors with `LexError::UnterminatedSubstitution`.
2. **`consume_paren_cmdsub_verbatim`** (added v166) — appends the body **verbatim**
   (including the closing `)`) to a `String`. Errors with
   `LexError::UnterminatedBrace`. Used by `scan_braced_operand`.
3. **`split_modifier_operand`** (added v165) — tracks `$(` paren depth inline,
   woven into its modifier-delimiter detection (the delimiter is ignored while
   `paren_depth > 0`).

The three differ only in: where the opening `$(` is consumed, whether the
closing `)` is included in the output, the error variant, and whether the body
is parsed vs appended vs skipped-for-splitting. The *scanning kernel* is
identical. Keeping three copies is the exact maintenance hazard that produced
L-10/L-52 (a fix to one loop's `$()` handling does not reach the others).

## Design

### The shared kernel

```rust
/// Scans a `$(…)` command-substitution body, the opening `$(` having already
/// been consumed by the caller. Consumes through the matching `)` (which is
/// consumed but NOT appended); any unquoted `(` raises the paren depth and any
/// unquoted `)` lowers it, so nested `$(…)`, `$((…))`, and `$( (…) )` balance;
/// `'…'` and `"…"` spans are skipped (double-quote honors `\`) and a `\` escapes
/// the next char — none of these affect depth. The body text (excluding the
/// closing `)`) is appended to `out`. Running out of input in any unterminated
/// position returns `Err(unterminated)`.
fn scan_cmdsub_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError>
```

The kernel is `scan_paren_substitution`'s current loop minus the redundant
`$`-special-case: a bare `(` already raises depth, so a `$` followed by `(`
produces the identical body text and depth whether or not `$(` is handled as a
unit. (Verified: `$x`, `$` at EOF, and `$(` all behave identically with the `$`
arm removed.)

### The three callers

- **`scan_paren_substitution`**:
  ```rust
  let mut body = String::new();
  scan_cmdsub_body(chars, &mut body, LexError::UnterminatedSubstitution)?;
  parse_substitution_body(&body, opts)
  ```
  Same body text, same error variant, same parse — behavior identical.

- **`consume_paren_cmdsub_verbatim`** (thin wrapper; caller already pushed `$(`):
  ```rust
  scan_cmdsub_body(chars, out, LexError::UnterminatedBrace)?;
  out.push(')');
  Ok(())
  ```
  The kernel excludes the closing `)`; this re-adds it for verbatim
  reconstruction — preserving v166's exact output and error variant.

- **`split_modifier_operand`**: in its `'$'` arm, when `$` is followed by `(`,
  push `$(` to the current destination segment and call
  `consume_paren_cmdsub_verbatim(chars, dst)`, **ignoring** its `Result`. Remove
  the inline `paren_depth` field, the `'(' if paren_depth > 0` / `')' if
  paren_depth > 0` arms, and simplify the delimiter guard from `… && paren_depth
  == 0 && brace_depth == 0 …` to `… && brace_depth == 0 …`.

  *Why ignoring the `Result` is safe:* `split_modifier_operand`'s input is an
  operand body already extracted by `scan_braced_operand`, which (since v166)
  itself consumes `$(…)` via `consume_paren_cmdsub_verbatim` and would have
  errored on an unterminated one. So `split_modifier_operand` never receives an
  unterminated `$(`. Even in the impossible case, the helper appends the partial
  body and leaves the cursor exhausted, so `split_modifier_operand`'s
  `while let Some(c) = chars.next()` loop ends with the same segments the old
  inline code produced — behavior-preserving.

### Behavior preservation

This is a pure refactor: identical body text, identical error variants (passed
in), identical termination, identical parse. For every input, output is
unchanged. The L-10/L-52 fixes (v165/v166) are preserved because their callers
now share the same kernel that already had the correct `$()` handling.

## Scope boundary (v164/v165/v166 lessons)

`$()` only. **Not** in scope: the analogous backtick triplication
(`scan_backtick_substitution` + the backtick arms in `scan_braced_operand` and
`split_modifier_operand`) — a separate, simpler span, noted as a possible v168
follow-on; the `arith.rs` `CharCursor`/`LexerOptions` migration (L-24); and any
behavior change. No `docs/bash-divergences.md` edit (no divergence resolved or
introduced).

## Verification

- **Unit test** for `scan_cmdsub_body` directly (in `src/lexer.rs`): a simple
  body, a nested `$( $() )`, `$((…))`, a `)` inside a quoted span, and an
  unterminated body → `Err(<the variant passed in>)`. Plus the existing
  `scan_braced_operand_*` and `split_modifier_operand_*` unit tests (v165/v166)
  must still pass unchanged — they are the behavior-preservation guard for two
  of the callers.
- **Full regression:** the whole unit suite, all integration tests, and all 92
  bash-diff harnesses stay green; `cargo clippy --lib --bins` clean. Every
  `$(…)` in every test flows through `scan_paren_substitution`, so the hot path
  is exercised broadly. Spot-check the L-10 (`${s/$(echo a/x)/Z}`) and L-52
  (`${s/$(echo a}b)/Z}`) cases against bash to confirm no regression in the
  sibling area.
- **Net check:** `src/lexer.rs` non-test line count should DROP (three loop
  bodies → one kernel + thin callers).

## Docs / iteration close-out

Pure refactor — record in `project_huck_iterations.md` + `MEMORY.md` only; mark
the DRY-unification follow-on done and note the backtick triplication as the
remaining analogous cleanup.
