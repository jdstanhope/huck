# v168: backtick-scan kernel — Design

**Status:** approved 2026-06-16
**Iteration:** v168
**Origin:** the v167 close-out noted that the backtick (`` `…` ``) command
substitution has the same triplication v167 just fixed for `$(…)`. This is the
direct mirror — one source of truth for backtick boundary scanning.

## Goal

Extract the backtick command-substitution **boundary scan** (advance to the
first un-escaped backtick, treating `\<any>` as an escaped pair) into one private
kernel, and route the three current copies through it, with no change in
observable behavior. Move the parse path's bash un-escaping into a small
dedicated function.

## Problem

Three functions in `src/lexer.rs` each scan a backtick body:

1. **`scan_backtick_substitution`** — the real backtick tokenizer. Applies bash's
   un-escaping (`` \` `` → `` ` ``, `\\` → `\`, `\$` → `$`, `\x` → `\x`) while
   scanning, then **parses** the resulting body via `parse_substitution_body`.
   Errors `LexError::UnterminatedSubstitution`.
2. **`scan_braced_operand`** backtick arm (added v166) — appends the body
   **raw** (escapes preserved) to reconstruct the `${…}` operand verbatim.
   Errors `LexError::UnterminatedBrace`.
3. **`split_modifier_operand`** backtick arm (added v165) — appends the body
   **raw** to the current split segment; no error path (its input is
   pre-balanced).

The differences are: un-escape vs raw output, whether the closing backtick is
appended, and the error variant. The *boundary scan* — "consume to the first
backtick not preceded by `\`, treating `\<any>` as an escaped pair" — is
identical in all three and quote-naive (backticks do not nest and quotes do not
protect the closing backtick; an inner backtick is escaped with `` \` ``). Three
copies of that boundary logic is the same drift hazard that produced L-10/L-52
in the `$()` scanners.

## Design

### The kernel

```rust
/// Scans a backtick (`` `…` ``) command-substitution body, the opening backtick
/// having already been consumed by the caller. Consumes through the matching
/// un-escaped backtick (which is consumed but NOT appended); a `\` escapes the
/// next char (so `` \` `` does not close the span — the backslash and the next
/// char are both appended raw). The body (raw, with escapes preserved, excluding
/// the closing backtick) is appended to `out`. Backticks are quote-naive and do
/// not nest. Running out of input returns `Err(unterminated)`. The single source
/// of truth for backtick boundary scanning.
fn scan_backtick_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError>
```

Loop: `None` → `Err(unterminated)`; `` ` `` → `Ok(())` (consumed, not appended);
`\` → append `\`, then append the next char (or `Err(unterminated)` at EOF);
any other char → append. (This is the verbatim arms' boundary logic, with the
error variant parameterized.)

### The verbatim wrapper

```rust
fn consume_backtick_verbatim(chars: &mut CharCursor<'_>, out: &mut String) -> Result<(), LexError> {
    scan_backtick_body(chars, out, LexError::UnterminatedBrace)?;
    out.push('`'); // re-add the closing backtick for verbatim reconstruction
    Ok(())
}
```

(Mirrors v167's `consume_paren_cmdsub_verbatim`.)

### The un-escape helper (parse path only)

```rust
/// Applies bash's backtick un-escaping to a raw backtick body: `` \` `` → `` ` ``,
/// `\\` → `\`, `\$` → `$`, and `\x` (any other char) → `\x` verbatim. A trailing
/// lone `\` is kept. This is the transform `scan_backtick_substitution` applies
/// before parsing; only the parse path un-escapes, so it lives in one function.
fn unescape_backtick(raw: &str) -> String
```

### The three callers

- **`scan_braced_operand`** backtick arm → `body.push('`'); consume_backtick_verbatim(chars, &mut body)?;` (replaces the inline loop; same raw output incl. opening/closing backtick, same `UnterminatedBrace`).
- **`split_modifier_operand`** backtick arm → `dst.push('`'); let _ = consume_backtick_verbatim(&mut chars, dst);`. The `Result` is ignored: the operand body is pre-balanced by `scan_braced_operand`, so an unterminated backtick is unreachable; on the impossible EOF the wrapper's `?` returns before `out.push('`')` (no spurious closing backtick) with the partial raw body already appended and the cursor exhausted, so the outer loop ends with identical segments — exactly v167's pattern.
- **`scan_backtick_substitution`**:
  ```rust
  let mut raw = String::new();
  scan_backtick_body(chars, &mut raw, LexError::UnterminatedSubstitution)?;
  parse_substitution_body(&unescape_backtick(&raw), opts)
  ```
  `unescape_backtick(&raw)` reproduces exactly the body the current inline loop
  builds (verified: the raw body with escapes preserved, transformed by the same
  `` \` ``/`\\`/`\$`/`\x` rules), and the error variant is preserved.

### Behavior preservation

Pure refactor: same body bytes at each call site (raw for the verbatim arms,
un-escaped for the parse path), same error variants (passed in / preserved),
same termination. Backticks remain quote-naive and non-nesting.

## Scope boundary

Backtick only. **Not** in scope: the `$()` kernel (done — v167), the `arith.rs`
`CharCursor`/`LexerOptions` migration (L-24), or any behavior change. No
`docs/bash-divergences.md` edit (no divergence resolved or introduced).

## Verification

- **Unit tests** in `src/lexer.rs`:
  - `scan_backtick_body`: a plain body (`` `echo hi` `` → raw `echo hi`, closing
    backtick consumed); an escaped backtick (`` a\`b` `` → raw `` a\`b ``, does not
    close early); a `\\` and `\$` raw-preserved; unterminated → `Err(<variant
    passed in>)` for both variants.
  - `unescape_backtick`: `` a\`b `` → `` a`b ``; `\\` → `\`; `\$` → `$`;
    `\x` → `\x`; trailing lone `\` kept.
  - The existing backtick behavior is also guarded by the v165/v166 unit tests
    (`scan_braced_operand_skips_backtick_cmdsub_with_brace`) which must still pass
    unchanged.
- **Full regression:** whole unit suite + all integration tests + all 92
  bash-diff harnesses green; `cargo clippy --lib --bins` clean. Spot-check
  against bash: a plain `` `echo hi` ``, an escaped inner backtick, a backtick in
  a `${…}` operand (the L-52 case `` ${s/`echo a}b`/Z} ``), and a backtick whose
  body contains `$VAR`/quotes.
- **Net check:** `src/lexer.rs` non-test line count should drop.

## Docs / iteration close-out

Pure refactor — record in `project_huck_iterations.md` + `MEMORY.md` only; note
the `$()`+backtick kernels are both done, leaving `arith.rs` (L-24) as the
remaining scanner-area follow-on.
