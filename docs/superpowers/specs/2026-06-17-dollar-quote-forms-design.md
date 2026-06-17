# v181: `$`-quote forms (`$'…'` ANSI-C, `$"…"` locale) in the lexer — Design

**Status:** approved 2026-06-17
**Iteration:** v181
**Origin:** The parse sweep's `fcnal-test.sh` gap (tracked as **L-48**, added during v180).
The L-48 entry blamed a single-quoted `awk` C-style-for body inside `$(…)`; root-cause
investigation proved that WRONG. The real cause is a lexer bug: inside a double-quoted
string huck treats `$'` as the start of ANSI-C `$'…'` quoting and scans off the end →
`unterminated quote`. In `fcnal-test.sh` line 187 the regex `"…ping6)$'"` ends in `$`
immediately followed by a literal `'`, hitting exactly this; the mis-parse then derails
the lexer and produces the cascading spurious "unexpected '}'" / "invalid variable name
in 'for' loop" errors at later lines (348, 712) — those constructs parse cleanly in
isolation (cascade confirmed). A sibling gap in the same lexer arm: `$"…"` (bash's
locale-translation quoting) is not implemented at all — `echo $"hello"` prints `$hello`
instead of `hello`.

## Problem

`scan_dollar_expansion` (`src/lexer.rs:1747`) is the shared kernel all word scanners use
for `$…`. It takes a `quoted: bool` (true when the `$` was found inside a double-quoted
string). Two arms mishandle the `$`-quote forms:

1. **`$'` ignores `quoted`** (lines 1785-1789):
   ```rust
   Some('\'') => {
       chars.next();
       let text = scan_ansi_c_quoted(chars)?;
       parts.push(WordPart::Literal { text, quoted: true });
   }
   ```
   This enters ANSI-C scanning regardless of context. bash only treats `$'…'` as ANSI-C
   quoting **outside** double quotes; inside `"…"` the `$` is a literal character. So
   `echo "$'"` (bash prints `$'`) makes huck scan from after `$'` for a closing `'`,
   running past the closing `"` to EOF → `LexError::UnterminatedQuote`.

2. **`$"` has no arm** — it falls to the `_` arm (literal `$`), so the following `"…"` is
   parsed as an ordinary double-quoted string and the `$` is left in the output:
   `echo $"hello"` → `$hello` (bash: `hello`).

Confirmed vs bash (C locale):
- `echo "$'"` → bash `$'`; huck `unterminated quote` error.
- `echo "a$'b"` → bash `a$'b`; huck error.
- `x="cost $'n"; echo "$x"` → bash `cost $'n`; huck error.
- `echo $"hello"` → bash `hello`; huck `$hello`.
- `msg=$"hi there"; echo "$msg"` → bash `hi there`; huck `$hi there`.
- `echo $"a $x b"` (x=Z) → bash `a Z b` (expansions happen inside `$"…"`); huck `$a Z b`.

Both forms are bash-special **only when not already inside double quotes**.

## Goal

Match bash for both `$`-quote forms: `$'…'` is ANSI-C quoting only outside double quotes
(inside, `$'` is a literal `$`); `$"…"` is locale-translation quoting only outside double
quotes — and since huck has no message catalog, the translation is the **identity**, so
`$"…"` ≡ `"…"`. This resolves L-48 (and clears `fcnal-test.sh`) and fixes the `$"…"`
output divergence.

## Design

Both changes are in `scan_dollar_expansion`'s `match chars.peek()`.

### 1. `$'` — guard with `if !quoted` (crash fix)

```rust
        Some('\'') if !quoted => {
            chars.next();
            let text = scan_ansi_c_quoted(chars)?;
            parts.push(WordPart::Literal { text, quoted: true });
        }
```
When `quoted` (inside double quotes) the arm is skipped and execution reaches the `_`
arm, which pushes a literal `$` (with the `quoted` flag) **without consuming the `'`**.
The caller's double-quote loop (`src/lexer.rs:568`, `Some(ch) => quoted_current.push(ch)`)
then pushes the `'` as a literal. Net: literal `$` + literal `'` = `$'`, matching bash.

### 2. `$"` — add a `!quoted` "drop the `$`" arm (locale translation = identity)

```rust
        Some('"') if !quoted => {
            // `$"…"` is bash's locale-translation quoting. huck has no message
            // catalog, so the translation is the identity: `$"…"` ≡ `"…"`. Drop the
            // `$` and leave the `"` unconsumed so the caller's existing double-quote
            // handler scans the body with its normal expansions/escapes. (Inside double
            // quotes — `quoted` — `$"` is instead a literal `$`, handled by the `_`
            // arm, after which the `"` closes the surrounding string; matching bash.)
        }
```
The arm body is intentionally empty: it consumes nothing and pushes nothing, so the `$`
is dropped and control returns to the caller, whose next loop iteration reads the `"` and
runs its normal double-quote handler (main word loop `src/lexer.rs:536`; no-brace-expansion
loop `src/lexer.rs:1178`). Both `!quoted` callers have a following `"` handler, so the body
`"…"` is parsed exactly as a plain double-quoted string — identity translation. This
reuses the entire double-quote scanner (DRY) and adds no duplicate body-scanning code.

### Why `quoted == true` needs no new code

Inside double quotes bash treats both `$'` and `$"` as a literal `$` followed by the
quote char. With both new arms gated on `!quoted`, a `quoted` `$'`/`$"` falls to the
existing `_` arm (`parts.push(Literal "$")`, no char consumed); the double-quote loop then
handles the trailing `'` as a literal char or the trailing `"` as the string terminator —
both already correct. (Verified: `echo "$'"` and `echo "a$\"b"` already behaved correctly
for `$"`; only `$'` crashed.)

### Behavior after the fix

- `echo "$'"` → `$'`; `echo "a$'b"` → `a$'b`; `x="cost $'n"; echo "$x"` → `cost $'n`.
- `echo $'a\tb'` (unquoted ANSI-C) → still decodes the tab (unchanged — `!quoted` path).
- `echo $"hello"` → `hello`; `echo $"a $x b"` (x=Z) → `a Z b`; `echo $""` → empty arg.
- `fcnal-test.sh`: `huck -n` emits no diagnostics, rc 0 = bash (all three cascade errors
  clear).

## Verification

- **New bash-diff harness** `tests/scripts/dollar_quote_forms_diff_check.sh` (executing,
  byte-identical bash↔huck stdout+exit). Cases: `$'`-inside-double-quotes —
  `echo "$'"` (→ `$'`), `echo "a$'b"` (→ `a$'b`), the real end-of-segment shape
  `echo "ping6)$'"` (→ `ping6)$'`), assignment `x="cost $'n"; echo "$x"` (→ `cost $'n`),
  and the nested-quote-switch shape from fcnal-test
  `echo 'a "b'"'c)$'"'d'` (adjacent single/double/single concatenation, `$'` inside the
  double-quoted middle); `$"…"` — `echo $"hello"` (→ `hello`), `x=Z; echo $"a $x b"`
  (→ `a Z b`, expansion happens), `echo $""` (→ empty arg),
  `echo $"with \"escaped\" and $x"`; controls — unquoted `echo $'a\tb\nc'` ANSI-C escapes
  still decode, plain `echo "x"` / `echo 'y'` unaffected. Compare stdout+exit (all print
  clean output; no intentional-stderr cases).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh tools/scripts.tsv`; confirm
  `fcnal-test.sh` now parses silently (`huck -n` rc 0, no stderr) and report `HUCK_GAP`
  movement from the 26 baseline; `HUCK_LENIENT`/`HUCK_CRASH` stay 0.
- **Full `cargo test`** (0 failures). UP-FRONT (v178/v180 lesson) grep all of `tests/` +
  `src/` for tests encoding the OLD buggy behavior — any asserting `$"…"` yields a leading
  `$` (e.g. `$hello`), or any lexer unit test feeding `$'` inside double quotes. Update
  those to the corrected behavior; do not weaken unrelated tests. Add a lexer unit test:
  `tokenize("\"$'\"")` yields the literal `$'` (a quoted Literal), and `tokenize("$\"x\"")`
  yields a single quoted Literal `x` (no leading `$`).
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

**Resolves L-48** (the root cause was misidentified there): **delete** the L-48 entry from
`docs/bash-divergences.md`. No new deferred entry (the fix is complete — both forms match
bash; identity-only translation is the correct behavior for a shell with no i18n, not a
gap). Record the iteration in `project_huck_iterations.md` + `MEMORY.md`, and CORRECT the
sweep-memory note that attributed the `fcnal-test` gap to awk-in-cmdsub.

## Scope boundary

In scope: the two arms in `scan_dollar_expansion` (`$'` guard, `$"` drop-arm), the new
harness + lexer unit tests, the L-48 deletion, updating any old-behavior tests. **Not** in
scope: real gettext/i18n message-catalog translation (identity is correct and complete for
huck); `scan_ansi_c_quoted` / `decode_ansi_c_escape` (unchanged — the escape semantics are
correct); command-substitution / arithmetic body scanning; the `${…}` and `$(…)` arms; the
error-message wording family. No behavior change to unquoted `$'…'` or to `$"`/`$'` already
inside double quotes (the latter now correct via the `_` arm).
