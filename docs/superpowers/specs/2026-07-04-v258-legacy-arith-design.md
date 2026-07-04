# v258 — `$[ expr ]` legacy arithmetic expansion on the atom-command path

**Status:** design approved (2026-07-04)
**Arc:** Phase C, Stage 2 — porting deferred construct families onto the dormant
atom-command parser (`command_atoms`, default `false`), one family per iteration,
byte-identical to the `command.rs` oracle. **This is the LAST deferred family; after
it the finale (flip `command_atoms` live + delete the forward-scanning scanners)
becomes reachable once the accumulated live-flip carry-forwards are cleared.**

## Summary

Port `$[expr]` (legacy/deprecated arithmetic expansion — bash treats it exactly as
`$((expr))`) onto the atom path. Today `$[` emits a zero-width `DeferredExpansion`
signal at the dollar-dispatch and the parser defers (`UnsupportedExpansion` /
`UnsupportedCommand`). v258 makes it a real `WordPart::Arith`, byte-identical to
`$((expr))`.

**Dormant + differential.** `command_atoms` stays `false`. `command.rs` is
EMPTY-diff (the oracle already handles `$[` via `scan_legacy_arith_body`). The atom
path builds the SAME `WordPart::Arith { body, quoted }` AST as `$((expr))`.

**Chosen approach — extend `Mode::Arith` with a delimiter field (Approach A, the
v256 `for_header` pattern).** `$[expr]` is the *same arith body* as `$((expr))`,
differing only in the delimiter: track `[`/`]` depth instead of `(`/`)`, close on a
single depth-0 `]` instead of `))`, and treat `(`/`)` as literal body chars. The
whole `$`/`${`/`$(`/backtick/special-param body-emission block is shared untouched.
(Rejected Approach B — a separate `Mode::LegacyArith` + `scan_step_legacy_arith` —
would duplicate that block or require extracting it; Approach A reuses it with the
delimiter localized to a few arms.)

## Background — the oracle (probed)

The oracle Word-lexer's `scan_legacy_arith_body` (lexer.rs:5407) FORWARD-scans `$[
… ]` to the matching `]` (tracking raw `[`/`]` nesting; protecting `'…'`/`"…"`
verbatim and `$(…)`/`${…}` via `scan_cmdsub_body`/`scan_braced_skip`), then feeds
`arith_string_to_word` — producing `WordPart::Arith` identical to `$((expr))`. That
forward scan (and `arith_string_to_word`) is exactly what THE RULE forbids and the
finale deletes, so the atom path must be atom-native.

**Probed oracle ASTs (the differential targets):**
- `echo $[1+2]` → `Arith { body: Word([Literal{"1+2", quoted:true}]), quoted:false }`
  — identical to `echo $((1+2))`.
- `echo pre$[1+2]post` → `[Literal "pre", Arith{"1+2"}, Literal "post"]`.
- `echo $[a[0]]` → `Arith{body:"a[0]"}` (inner `[0]` bracket-nested; outer `]` closes).
- `echo $[(1+2)*3]` → `Arith{body:"(1+2)*3"}` (parens are literal body chars).
- `echo $[ x + 1 ]` → `Arith{body:" x + 1 "}` (whitespace preserved).
- `echo "$[1+2]"` → `Quoted{Double, [Arith{"1+2", quoted:true}]}`.
- `echo $[1+2` (unterminated) → LEX error (`UnterminatedLegacyArith`) → `old_seq`
  panics (`.expect("lex")`), so it is NOT `diff_err`-testable.

## Architecture

**Files:**
- `crates/huck-syntax/src/lexer.rs` — `ArithDelim` enum; `delim` field on
  `Mode::Arith`; `TokenKind::LegacyArithOpen`; `scan_step_arith` parametrized on
  `delim`; the two `$[`→`LegacyArithOpen` dollar-dispatch arms.
- `crates/huck-syntax/src/parser.rs` — `parse_legacy_arith_expansion`; a
  `LegacyArithOpen` arm parallel to every `ArithOpen` arm; the `delim: Paren`
  fan-out at the 3 push sites; deferral-test flips + new corpus.
- `crates/huck-syntax/src/command.rs` — UNTOUCHED (EMPTY diff).

### Lexer

- **`enum ArithDelim { Paren, Bracket }`** (`Copy`, `Eq`).
- **`Mode::Arith { paren_depth, in_dquote, body_started, for_header, delim }`.** All
  existing construction sites (3 parser pushes at parser.rs:1277/1330/2918; the
  lexer match at :957; the `scan_step_arith` field reads; the ~8 test sites) set
  `delim: ArithDelim::Paren` — mechanical, byte-unchanged. Only the new `$[` push
  uses `Bracket`.
- **`TokenKind::LegacyArithOpen`** — the `$[` opener signal. The CLOSE reuses
  `ArithClose` (so `parse_arith_body` needs no change).
- **Dollar-dispatch** (lexer.rs:3132 dquoted `quoted:true`, 3838 unquoted
  `quoted:false`): the `Some('[')` arms emit `LegacyArithOpen` (zero-width, cursor
  stays on `$`) instead of `DeferredExpansion`. The other `DeferredExpansion`
  emissions (in-dquote `$(`/`$((`) are untouched.
- **`scan_step_arith` parametrized on `delim`:**
  - `!body_started`: `Paren` consumes `$((` (3 chars) + emits `ArithOpen`;
    `Bracket` consumes `$[` (2 chars) + emits `LegacyArithOpen`.
  - Body loop: the hardcoded `(`/`)` arms become `open_char`/`close_char` per
    `delim` (`(`/`)` for Paren, `[`/`]` for Bracket). Close-at-depth-0: `Paren`
    keeps the `peek_nth(1)==')'` `))`-check + `ArithClose`/`ArithBail`; `Bracket`
    emits `ArithClose` on a single depth-0 `]` (NO bail — `$[` has no `$( (`
    wrinkle). The non-delimiter bracket/paren falls into the existing catch-all
    `Some(ch) => text.push(ch)` as a literal (`(`/`)` literal in Bracket; `[`/`]`
    literal in Paren already works). The shared `$`/`${`/`$(`/backtick/
    special-param block is UNTOUCHED.
  - EOF with no close → `UnterminatedArith` (existing). Optionally emit
    `UnterminatedLegacyArith` for `Bracket` to match the oracle's error kind (both
    are `ParseError::Lex`; dormant, so low-stakes — match if cheap).

### Parser

- **`parse_legacy_arith_expansion(iter, quoted)`** (mirrors `parse_arith_expansion`
  WITHOUT the bail/mark/rewind):
  ```
  push Mode::Arith { paren_depth:0, in_dquote:quoted, body_started:false, for_header:false, delim:Bracket }
  match next_kind()? { Some(LegacyArithOpen) => {}, _ => Err(UnsupportedExpansion) }
  let outcome = parse_arith_body(iter, quoted)?      // reused; Closed on ArithClose
  pop_mode
  match outcome { Closed(body) => WordPart::Arith { body, quoted }, Bail => unreachable/Err }
  ```
  No `mark`/`rewind` (no bail path for `$[`).
- **`LegacyArithOpen` arm parallel to every `ArithOpen` arm** (parser.rs:87, 204,
  324, 437, 1248 [nested, incl. nested `$[`], 1483 [heredoc], 2887, and the atom-set
  membership at :1936): peek `LegacyArithOpen` → `iter.next_kind()?` (discard the
  zero-width signal) → `parse_legacy_arith_expansion(iter, quoted)`. This uniformly
  covers command position, dquote, nested arith, heredoc body (v250), regex (v254),
  array values, case, `for ((` header — closing all accumulated `$[expr]`
  carry-forwards.

### Progress / OOM safety

`scan_step_arith` makes progress every call (consumes a char or emits a token). No
mark/rewind, no non-progress loop. `parse_legacy_arith_expansion` is a bounded
push→body→pop.

## Differential corpus

All `diff_cmd` (`$[…]` → `WordPart::Arith`) unless noted. Every value probed against
the oracle before the plan is finalized.

**Base:** `echo $[1+2]`, `echo pre$[1+2]post`, `echo $[ x + 1 ]`, `echo $[a[0]]`,
`echo $[(1+2)*3]`, `x=$[1+2]`.
**Embedded expansions:** `echo $[$x+1]`, `echo $[${a}+1]`, `echo $[$(echo 1)+2]`,
`` echo $[`echo 1`+2] ``, `echo $[$((1+2))+3]` (nested `$((`), `echo $[$[1+2]+3]`
(nested `$[`).
**Quoted context:** `echo "$[1+2]"`, `echo "pre$[1+2]post"`.
**Carry-forward sites (close the deferrals):** `$[1+2]` in an expanding heredoc body
(v250), `[[ x =~ $[1+2] ]]` (regex, v254), `a=($[1+2])` (array value), `case $[1+2]
in a) :; esac`, `$[` in a `for (( … ))` header (v256).
**Pinned edges (inherited from `$((`, `diff_cmd` if they happen to still match, else
a pinned carry-forward test):** `echo $[ "]" ]`, `echo $[ \] ]` — the atom closes
early (quotes/`\` are literal, no sub-mode), exactly as `$(( ")" ))` / `$(( \) ))`.
Probe `$((`'s behavior first to confirm the inheritance and pin accordingly.
**Unterminated:** `echo $[1+2` — lex error on both; tokenize-level check that both
error (not `diff_err`; `old_seq` panics).

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd`.
- `command.rs` diff-vs-main = EMPTY.
- `lexer.rs` diff = the `delim` field + `ArithDelim` + `LegacyArithOpen` +
  `scan_step_arith` parametrization + the two dispatch arms. The ENTIRE existing
  arith suite (`$((`/`((`/`for ((`) is the regression net for `Paren`
  byte-unchanged.
- Both `command_atoms` sites (lexer.rs:811/812, 4167/4183) stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green.
- `cargo build -p huck-syntax` → 0 warnings.

## Task decomposition

- **T1 — lexer:** `ArithDelim` + `delim` on `Mode::Arith` (all sites → `Paren`) +
  `LegacyArithOpen` + `scan_step_arith` parametrization (`open_char`/`close_char`
  per `delim`; `Bracket` close → single-`]` `ArithClose`, no bail) + the two
  `$[`→`LegacyArithOpen` dispatch arms. Focused lexer test (a `Bracket` body emits
  `LegacyArithOpen` + body Lit + `ArithClose`); the existing arith suite is the
  `Paren` byte-unchanged gate.
- **T2 — parser core:** `parse_legacy_arith_expansion` + a `LegacyArithOpen` arm
  parallel to every `ArithOpen` arm; base + embedded + quoted + assignment corpus;
  flip the `$[expr]` deferral tests (parser.rs:4459 and any regex/heredoc
  carry-forward deferral tests).
- **T3 — carry-forward sites + edges:** heredoc / regex / array / case /
  arith-for-header `$[`; the quote/backslash-protection pins; the unterminated note;
  adversarial corpus.

## Live-flip carry-forwards

Anticipated: the quote/backslash delimiter-protection edges (`$[ "]" ]` / `$[ \] ]`),
inherited from `$((`'s literal quote/`\` handling — pinned, dormant. Any additional
divergence found during implementation is pinned with a test and recorded in the
ledger. **This iteration completes the deferred-family ports; the remaining pre-flip
work is clearing the accumulated live-flip carry-forwards (heredoc-in-cmdsub attach,
array over-accept, even-bang-before-compound, arith-body quote retention, the
`$((`/`$[` delimiter-protection edges, etc.), then the finale.**
