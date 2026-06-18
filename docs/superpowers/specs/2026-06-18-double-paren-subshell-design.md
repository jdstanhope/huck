# v184: `((` command — arithmetic vs nested subshell disambiguation — Design

**Status:** approved 2026-06-18
**Iteration:** v184
**Origin:** The parse sweep's "unterminated `((` arithmetic block" cluster (3
files, one root cause): `/usr/bin/zdiff` (`((gzip -cdfq … ) … ) 5<&0` inside a
`$(…)`) and `kselftest/runner.sh` ×2 (`(((((tap_timeout … ) | …)`). All are a
`((` at command position that is actually NESTED SUBSHELLS `( (…) )`, which huck
commits to as an arithmetic block and then fails to close.

## Problem

When the lexer sees `((` (two adjacent `(` at a command/word position), it
consumes the second `(` and calls `scan_arith_block` (`src/lexer.rs:1994`),
which scans to a depth-0 `))`. On success it emits `Token::ArithBlock`; on
failure it returns `LexError::UnterminatedArithBlock`, which propagates via `?`
at `src/lexer.rs:725` — a hard parse error.

bash disambiguates differently: `((` is an arithmetic command ONLY if a matching
`))` is found; otherwise it is `( (` — a subshell containing a subshell. Verified
against bash 5.x:

| fragment | bash | huck (today) |
|---|---|---|
| `((echo a) \| cat)` | `a` (nested subshells, no `))`) | ★ unterminated `((` |
| `(((  echo a ) ) )` | `a` | ★ unterminated `((` |
| `((1+2)) && echo t` | `t` (arith, has `))`) | `t` |
| `((x=5)); echo $x` | `5` | `5` |
| `((echo hi))` | error (`))` found → arith → bad expr) | error (arith → bad expr) |
| `((echo a)(echo b))` | error (`))` found → arith) | error |

So bash's rule == "does a matching `))` exist": no `))` → nested subshells; `))`
present → arithmetic (which then errors if the body isn't valid arithmetic, same
as huck). This is exactly what `scan_arith_block` already tests.

## Goal

Match bash: when `scan_arith_block` finds no closing `))`, fall back to parsing
`((` as `( (` nested subshells instead of hard-erroring. This clears the cluster
and is purely additive (only inputs that currently hard-error change).

## Design

Mirror the v177 `$((` disambiguation (try arithmetic; on failure rewind and
reparse). At the `((` lexer site (`src/lexer.rs:723-729`), the current code is:

```rust
                if chars.peek() == Some(&'(') {
                    chars.next(); // consume the second `(`
                    let body = scan_arith_block(&mut chars)?;
                    tokens.push(Token::ArithBlock(body, opts));
                } else {
                    tokens.push(Token::Op(Operator::LParen));
                }
```

Replace with a save / try / rewind-on-failure:

```rust
                if chars.peek() == Some(&'(') {
                    // `((` is an arithmetic command ONLY if a matching `))` is
                    // found; otherwise bash treats it as nested subshells `( (`.
                    // Save the cursor at the second `(`, try the arith block, and
                    // on failure rewind + emit a single LParen (the first `(`); the
                    // second `(` then re-lexes as another LParen. A `((` that DOES
                    // close as `))` but isn't valid arithmetic stays an ArithBlock
                    // → arith error at parse/eval, matching bash. Mirrors the v177
                    // `$((` disambiguation.
                    let saved = chars.clone();
                    chars.next(); // consume the second `(`
                    match scan_arith_block(&mut chars) {
                        Ok(body) => tokens.push(Token::ArithBlock(body, opts)),
                        Err(_) => {
                            *chars = saved;
                            tokens.push(Token::Op(Operator::LParen));
                        }
                    }
                } else {
                    tokens.push(Token::Op(Operator::LParen));
                }
```

The shared `push_pos!(c_off, c_line)` (`:730`) still runs once for the single
emitted token (ArithBlock or the first LParen); the second `(` gets its own
position when re-lexed on the next loop iteration.

### Why this is correct and safe

- bash's `((`-is-arithmetic rule is precisely "a matching `))` exists", which is
  what `scan_arith_block` returns `Ok`/`Err` on. So `Ok` → arith (unchanged),
  `Err` → nested subshells (the fix).
- **Purely additive:** the only behavior change is for inputs where
  `scan_arith_block` currently returns `Err` (today a hard error). No currently-
  parsing input changes. Real arith (`((1+2))`, `((x=5))`) and the both-error
  cases (`((echo hi))`, which have `))`) are unaffected.
- **Deeper nesting peels correctly:** `(((` → outer `((` tries arith, fails (no
  `))`), rewinds, emits one LParen; the loop re-reads the second `(`, sees the
  third `(`, tries arith again, etc. — one `(` per failed attempt.
- The `( (` token stream parses as a subshell containing a subshell via the
  existing parser (no parser/executor change).

### Behavior after the fix

- `((echo a) | cat)` → `a`; `(((  echo a ) ) )` → `a`; the zdiff / runner.sh
  nested-subshell-with-redirections constructs parse.
- `((1+2))`, `((x=5)); echo $x` → unchanged.
- `((echo hi))` → still an arith error (matches bash erroring; wording differs by
  the intentional prefix convention).

## Verification

- **New bash-diff harness** `tests/scripts/double_paren_subshell_diff_check.sh`
  (executing, byte-identical bash↔huck stdout+exit): nested subshells `((echo a)
  | cat)` → `a`; with redirections (a zdiff-shaped `((cmd >…) … | cmd) <…`);
  deeply nested `(((  echo a ) ) )` → `a`; a subshell-in-subshell that sets no
  outer var; and ARITH controls `((1+2)) && echo t` → `t`, `((x=5)); echo $x` →
  `5`, `((n=3)); ((n++)); echo $n` → `4`. All produce clean identical stdout. (The
  both-reject `((echo hi))` is NOT byte-identical — error wording differs — so it
  is covered by a lexer unit test, not the harness.)
- **Lexer unit tests** (`src/lexer.rs` `mod tests`): `tokenize("((echo a) | cat)")`
  contains NO `Token::ArithBlock` and begins with two `Token::Op(Operator::LParen)`;
  `tokenize("((1+2))")` still yields a `Token::ArithBlock` (real arith unchanged);
  `tokenize("(((echo a)))")`-style deep nesting yields LParens, not an ArithBlock.
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh`; confirm zdiff +
  runner.sh ×2 parse (`huck -n` rc 0, no stderr — note any that fail on a
  DIFFERENT construct as a derail). Report `HUCK_GAP` from the 11 baseline;
  `HUCK_LENIENT`/`HUCK_CRASH` stay 0.
- **Full `cargo test`** (0 failures). UP-FRONT grep `tests/` + `src/` for tests
  asserting `((`-at-command-position behavior (esp. the existing arith-block /
  `[[ ((` tests at `:3733`-`:3793`) — confirm they still hold (the `[[ ((`
  suppression and real arith blocks are unchanged); update only genuine
  old-behavior tests (none expected).
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

No tracked `M-*`/`L-*` divergence covers this (sweep-found). No
`bash-divergences.md` change. Record the iteration in
`project_huck_iterations.md` + `MEMORY.md`; update the backlog note.

## Scope boundary

In scope: the `Err`-fallback at the `((` lexer site, the new harness + lexer
unit tests. **Not** in scope: `scan_arith_block`'s internals (quote/`$()`
blindness in its depth tracking is pre-existing and unchanged); the `$((`
expansion path (v177); the `[[ ((` arith-block suppression; the runtime arith
engine; the other sweep clusters (`unterminated 'case`, `unexpected token after
command`, `parameter expansion with empty name`, `function definition`). No
`bash-divergences.md` change.
