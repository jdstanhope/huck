# v169: fix L-24 — inherit extglob in arithmetic-nested command substitutions — Design

**Status:** approved 2026-06-16
**Iteration:** v169
**Origin:** L-24 (`[intentional]`, low). A command substitution nested inside
`$(( … ))` / `(( … ))` re-lexes with `extglob` OFF even when `shopt -s extglob`
is on, so an extglob pattern inside it fails to lex. This is the last
scanner-area follow-on after the v167/v168 kernels.

## Goal

Thread the active `LexerOptions` (currently just `extglob`) into the
arithmetic-body lexer so a command substitution nested inside arithmetic
inherits the parent's extglob state.

## Problem (verified against the current binary)

`arith_string_to_word` (`src/lexer.rs:1652`) lexes an arithmetic body into a
`Word`, re-lexing any nested `$(…)` / backtick command substitution. It passes
`LexerOptions::default()` (extglob OFF) at four sites — `read_dollar_expansion`
(`src/lexer.rs:1668`, `:1710`) and `scan_backtick_substitution`
(`src/lexer.rs:1673`, `:1715`) — instead of the parent's options. So extglob is dropped at the arithmetic
boundary. Two reproductions (bash succeeds, huck errors):

```
shopt -s extglob; echo $(( `ls -d /tmp/X@(a|b) 2>/dev/null | wc -l` ))   # bash: a count; huck: syntax error
shopt -s extglob; echo $(( $( [[ foo == @(foo|bar) ]] && echo 1 ) ))     # bash: 1; huck: unterminated '[[ ]]'
```

There are two call paths into `arith_string_to_word`:

- **Path A — `$(( … ))`** (`src/lexer.rs:1741`): inside the lexer, which **has
  `opts` in scope**. Both reproductions above go through this path.
- **Path B — `(( … ))` standalone / `for ((;;))`**: the lexer emits a
  `Token::ArithBlock(String)` (created at `src/lexer.rs:713`), and the **parser**
  (`src/command.rs`) later re-lexes the text via `arith_string_to_word`. The
  parser has no `LexerOptions`, so the token must carry it.

`LexerOptions` (`src/lexer.rs:306`) is `{ pub extglob: bool }` — `Copy`,
`Default`.

## Design

### Core: thread `opts` through `arith_string_to_word`

```rust
pub(crate) fn arith_string_to_word(s: &str, opts: LexerOptions) -> Result<Word, LexError>
```

Replace all four internal `LexerOptions::default()` with `opts` (lexer.rs:1668,
1673, 1710, 1715 — the `read_dollar_expansion` / `scan_backtick_substitution`
calls, including the two inside the quoted-span branch).

### Path A — `$(( … ))`

`src/lexer.rs:1741`: `arith_string_to_word(&inner, opts)` (the surrounding lexer
code already holds `opts`, used a few lines down for
`scan_paren_substitution(chars, opts)`).

### Path B — carry `opts` on the `ArithBlock` token

Change the token to carry the options captured at lex time:

```rust
// src/lexer.rs:286
ArithBlock(String, LexerOptions),
```

- Construction (`src/lexer.rs:713`): `tokens.push(Token::ArithBlock(body, opts))`
  (the `opts` active during tokenization is in scope there).
- Parser callers destructure and forward the stored opts:
  - `((expr))` at command position (`src/command.rs:1037`/`1040`):
    `let Some(Token::ArithBlock(text, opts)) = … ; arith_string_to_word(&text, opts)`.
  - `((x++))` in pipeline position (`src/command.rs:2246`/`2249`): same.
  - C-for header (`src/command.rs:1428`): `Some(Token::ArithBlock(text, opts)) => …`;
    thread `opts` through `parse_arith_for_header(text, opts)` →
    `parse_section` → `arith_string_to_word(trimmed, opts)`.
- Mechanical compiler-guided update of every other `Token::ArithBlock` pattern:
  `matches!(…, Token::ArithBlock(_))` → `Token::ArithBlock(..)` (command.rs:1036,
  1462, 1610, 2045, 2170, 2245; lexer.rs test sites 3646/3654/3663/3682/3706/7081)
  and `Token::ArithBlock(s) => assert_eq!(s, …)` → `Token::ArithBlock(s, _) =>`
  (lexer.rs test sites 7007/7017/7030/7040/7050). The compiler enumerates all of
  them; none carry behavior — they only match on the string.

### Why this is correct and contained

The only behavior change is that nested command substitutions inside arithmetic
now lex with the parent's extglob state instead of OFF — exactly the L-24 fix.
Storing `opts` on `ArithBlock` makes the parser-side re-lex (path B) see the same
options the lexer saw, mirroring how path A already had them. No other token or
AST type changes; `arith.rs`'s separate Pratt-parser tokenizer is untouched
(`arith_string_to_word` already uses `CharCursor`).

## Verification

- **New bash-diff harness** `tests/scripts/arith_extglob_diff_check.sh`, asserting
  byte-identical bash/huck output for, with `shopt -s extglob`:
  - Path A: `` echo $(( `… @(a|b) …` )) `` (backtick) and
    `echo $(( $( [[ foo == @(foo|bar) ]] && echo 1 || echo 0 ) ))`.
  - Path B: an extglob cmdsub inside `(( … ))` (e.g.
    `(( $( [[ x == @(x|y) ]] && echo 1 || echo 0 ) )); echo $?`) and inside a
    `for (( … ))` header section.
  - Control: the same fragments with extglob OFF should match bash too (both
    error or both treat `(` literally) — guards that the option is genuinely
    threaded, not forced on.
  This becomes the 93rd harness.
- **Unit test** in `src/lexer.rs`: `arith_string_to_word("$(case x in @(a|b)) echo 1;; esac)", LexerOptions { extglob: true })` succeeds (Ok) while the same body with `extglob: false` returns `Err` — pinning that the option is honored.
- **Full regression:** whole unit suite + all integration tests + all existing
  bash-diff harnesses green; `cargo clippy --lib --bins` clean. The compiler-driven
  `ArithBlock` pattern updates are covered by the existing arith/`(( ))`/C-for
  tests.

## Docs / iteration close-out

This resolves a real divergence: on merge, **delete** the L-24 entry from
`docs/bash-divergences.md` and decrement the Tier-4 count 41 → 40. Record the
iteration in `project_huck_iterations.md` + `MEMORY.md`; note that the
scanner-area follow-ons from the architecture review are now all done.

## Scope boundary

Only the `opts` threading + the `ArithBlock` token field. Not in scope: arith.rs's
Pratt-parser `Peekable<Chars>` tokenizer migration (unrelated to L-24), L-51, or
the `huck-glob`/`huck-syntax` crate extraction.
