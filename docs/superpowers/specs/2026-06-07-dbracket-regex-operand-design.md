# huck v105 — `[[ … =~ REGEX ]]` regex-operand lexing (M-100) Design

**Status:** approved design, ready for implementation plan.
**Implements:** lexing the right-hand operand of `=~` inside `[[ … ]]` as a single
literal **regex word** (parentheses / `|` / `((` are literal pattern text), so
real-world regexes like bash_completion's `[[ $option =~ (\[((no|dont)-?)\]). ]]`
parse and match. Today huck lexes those parens as shell grouping / an arithmetic
`((` block.
**Why now:** v104 made sourcing fast/deep enough to reach `bash_completion`
line 847, where huck currently dies with `unterminated '((' arithmetic block`
(the lexer grabs the regex `((` as an arith block and scans to EOF). 9 of
bash_completion's `=~` regexes use parens.
**Closes:** new bug **M-100** `[fixed v105]` (Tier-1).
**Branch (impl):** `v105-dbracket-regex-operand`.

## Root cause (verified)

huck's lexer is context-free for `[[ … ]]`: `[[`/`]]` are plain `Token::Word`s
(only the parser recognises them as keywords via `keyword_of`,
`src/command.rs:79`), and the body is tokenised with normal word/operator rules.
The `'('` arm (`src/lexer.rs:549`) turns a standalone `((` into a `Token::ArithBlock`
(scanning to the matching `))` — to EOF when none balances) and a single `(` into
`Token::Op(Operator::LParen)`, **purely by lexical adjacency, with no `[[ ]]`
awareness**. By the time the parser's `=~` handler (`parse_test_atom`,
`src/command.rs:2172`) runs, the regex's parens are already separate
`ArithBlock`/`LParen`/`RParen` tokens, and `next_test_word` (`src/command.rs:2018`)
rejects the leading `Op` with `TestExprMissingOperand`. The token stream is
destroyed before the parser sees it, so **the fix must be in the lexer.**

## Verified bash 5.2 contract (the operand grammar)

Probed directly; the design matches these exactly:

- Space **inside** `()` is part of the regex: `[[ "a b" =~ (a b) ]]` → match.
- Paren tracking is **naive** (no bracket-expression awareness): `[[ x =~ ([)]) ]]`
  is a bash **syntax error** (`unexpected token ')'`) because the `)` inside `[…]`
  still decrements the paren count. huck must replicate naive counting.
- Whitespace at paren depth 0 **ends** the operand; a second bare word at depth 0
  (`[[ a =~ a b ]]`) is a bash syntax error (huck will likewise reject it, since
  the test parser then expects `]]`/`&&`/`||`, not another word).
- The line-847 shape `(\[((no|dont)-?)\]).` parses cleanly under naive counting
  (`(` → \[ → `((` → `no|dont` → `)` → `-?` → `)` → \] → `)` → `.`, depth back to 0).
- `$var`/`${…}`/`$(…)` expansion still applies in the operand; `\` escapes the next
  char; quotes work. Brace expansion and extglob do **not** apply (`{`/`}`/`+(`
  are literal regex text).

## Section 1 — New lexer state (`src/lexer.rs`, `tokenize_core`)

Two fields, reset per `tokenize_core` call (so each tokenize is independent):

- `dbracket_depth: u32` — current `[[ ]]` nesting.
- `expect_regex: bool` — the next word is the `=~` operand.

**Maintenance** — right after a `Token::Word` is emitted (every
`emit_word_with_braces` site), inspect the just-emitted word:
- if it is a single **unquoted** `Literal` whose text is exactly `"[["` →
  `dbracket_depth += 1`;
- if exactly `"]]"` → `dbracket_depth = dbracket_depth.saturating_sub(1)`;
- if exactly `"=~"` **and** `dbracket_depth > 0` → `expect_regex = true`.

Only `=~` sets `expect_regex`; the regex scanner (Section 2) consumes-and-clears
it at the next word start, so no other word needs to touch it.

Use a small helper `unquoted_literal_text(&Word) -> Option<&str>` (Some when the
word is exactly one `WordPart::Literal { quoted: false, text }`) to classify
`[[`/`]]`/`=~` — this mirrors how `keyword_of` treats keywords (so quoted `'[['`
etc. are NOT treated as the keyword).

## Section 2 — Regex-operand scanning (`src/lexer.rs`)

In the main loop, at the point where a new token begins (after whitespace is
skipped and before the normal char dispatch): **if `expect_regex` is set**, clear
it and scan the regex operand instead of the normal dispatch, emitting exactly one
`Token::Word` (and one offset entry).

New `fn scan_regex_operand(chars: &mut CharCursor, …) -> Result<Vec<WordPart>, LexError>`
that reads the operand as a word, identical to normal word lexing **except**:

- Maintain `paren_depth: u32`. An unescaped, unquoted `(` pushes a literal `(` and
  `paren_depth += 1`; `)` pushes a literal `)` and `paren_depth = paren_depth.saturating_sub(1)`.
- `|`, `<`, `>`, `;`, `&` (unescaped, unquoted) are **literal** word characters
  (pushed to the literal run), NOT operators.
- Unquoted, unescaped **whitespace**: if `paren_depth == 0`, the operand ends
  (do not consume the whitespace — leave it for the main loop). If `paren_depth > 0`,
  it is a literal char of the regex (push it, including a literal newline).
- `\` escapes the next char: push it literally (a `\<newline>` is a line
  continuation — drop both and continue, matching the rest of the lexer; needed by
  bash_completion line 876 `=~ \<newline>regex`).
- Single quotes, double quotes, and `$`/`` ` `` expansions are handled **exactly as
  in the normal word path** — REUSE the existing helpers (the `$`-dispatch /
  `read_dollar_expansion`, the quote handlers) rather than re-implementing, so
  `$var`/`${…}`/`$(…)`/quoted spans produce the same `WordPart`s. (Quoted regex
  metacharacters are passed through as-is — see Divergences.)
- **No** brace expansion and **no** extglob: emit the operand as a plain `Word`
  (i.e. push `Token::Word(Word(parts))` directly, NOT via `emit_word_with_braces`,
  which would run brace expansion). `{`/`}`/`+(` etc. stay literal.
- EOF with the operand still open (e.g. `paren_depth > 0` at end of input, or an
  unterminated quote) → return the same `LexError` the normal word path would for
  an unterminated quote, so continuation/`UnterminatedDoubleBracket` still works
  (Section 4).

After scanning, push the `Word` token + its `token_start` offset, set
`has_token = false`, and continue the main loop. The very next significant token
will be the `]]` word (or `&&`/`||`), which adjusts `dbracket_depth` as usual.

## Section 3 — Parser: unchanged

`parse_test_atom`'s `=~` arm calls `next_test_word`, which now receives the regex
`Token::Word` (instead of a leading `Op`) and returns it as `TestExpr::Regex { lhs,
pattern: Word }`. Evaluation (`src/executor.rs:1221`) expands the Word and feeds it
to `regex::Regex` — all unchanged. **No `src/command.rs` change is required**; the
fix is entirely in the lexer.

## Section 4 — Interaction with existing `[[ ]]` features (must-not-regress)

- **Test grouping `[[ ( … ) ]]`** — only the operand *immediately after `=~`* is
  scanned in regex mode; a `(` anywhere else in `[[ ]]` still lexes to
  `Op(LParen)` and is parsed as grouping. Unaffected.
- **`(( ))` arith command / `$(( ))` expansion outside `[[ ]]`** — `expect_regex`
  is gated on `dbracket_depth > 0` and on the previous word being `=~`, so these
  paths are byte-unchanged.
- **v87 multi-line `[[ ]]`** — a regex ending at a newline at depth 0 simply ends
  the operand; the following newline + `]]` are handled by the existing
  `skip_test_newlines`. An operand left open at EOF (unterminated quote, or
  `paren_depth > 0`) must surface as a lex error that `continuation::classify`
  already maps to incomplete (`UnterminatedQuote`-style) or that the parser maps to
  `UnterminatedDoubleBracket`, so REPL continuation still prompts. Verify
  `[[ $x =~ foo` (no `]]`) and `[[ $x =~ (a` still request continuation rather than
  hard-erroring mid-line differently than today.
- **v90 extglob in `[[`** — extglob groups (`+(…)` etc.) are recognised by a
  prefix char before `(` (`src/lexer.rs:355`); in regex mode that path is bypassed
  (parens literal), which is correct: inside a `=~` regex, `+(a|b)` is literal ERE,
  not a shell extglob. Confirm `shopt -s extglob` does not change `=~` operand
  lexing.
- **v92 bare-word `[[ word ]]`** — no `=~`, so `expect_regex` never set. Unaffected.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | `dbracket_depth`/`expect_regex` state in `tokenize_core`; `unquoted_literal_text` helper; word-emit hook to update the state; `scan_regex_operand` + the `expect_regex` branch at token start |
| `tests/dbracket_regex_integration.rs` | NEW — operand lexing/matching cases incl. the bash_completion shapes |
| `tests/scripts/dbracket_regex_diff_check.sh` | NEW — 30th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-100 `[fixed v105]`; changelog; README row; quoted-literal L-note |

## Testing

1. **Lexer unit tests**: `[[ x =~ (a) ]]` lexes the operand as ONE `Word` (`(a)`),
   not `Op(LParen)`+…; `((a))`, `(a(b))`, `(\[((no|dont)-?)\]).` each one Word;
   `[[ ( -n x ) ]]` still lexes `(`/`)` as `Op` (grouping); `(( 1 ))`/`$(( 1 ))`
   outside `[[ ]]` unchanged; `[[ '[[' = x ]]` (quoted) does not change depth.
2. **Integration / match semantics** (vs bash, `2>&1`): each bash_completion regex
   shape matches the same strings as bash —
   - `[[ "a b" =~ (a b) ]]` → match; `[[ ab =~ (a b) ]]` → no match.
   - `[[ "[no-]" =~ (\[((no|dont)-?)\]). ]]` and the exact line-847 `if`.
   - `[[ $cur =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]` with a sample `$cur`.
   - `[[ "x]" =~ (-[^]]+) ]]`-style bracket-with-`]]`-inside.
   - `$var` interpolation inside the operand expands (e.g. `re='(a|b)'; [[ a =~ $re ]]`).
   - depth-0 trailing space then `]]` ends the operand; `&&`/`||` after `]]` work.
3. **bash-diff harness** `tests/scripts/dbracket_regex_diff_check.sh` (30th):
   deterministic match/no-match fragments printing `yes`/`no`, byte-identical to
   bash 5.2 — include the line-847 shape, `(a b)` space-in-parens, `[^]]`, and an
   alternation `^\~.*|^\/.*`.
4. **Regression**: full suite (2676+), all 29 existing harnesses, and **the real
   payoff** — sourcing `/usr/share/bash-completion/bash_completion` no longer emits
   `unterminated '(('` / `missing operand` at line 847 (report the next gap, if any).
5. **Must-not-regress spot checks**: `[[ ( -n a ) || -n b ]]`, `(( x=1 ))`,
   `echo $((1+1))`, `[[ x == a* ]]`, multi-line `[[ … =~ … \n ]]`.

## Edge cases & notes

- **Quoted-literal matching divergence (pre-existing, out of scope)**: bash matches
  a *quoted* substring of the regex literally (escaping regex metacharacters);
  huck expands the Word and passes the result to `regex::Regex`, so quoted
  metachars stay active. This fix does not change that; bash_completion's regexes
  use `\"`/`\$` escapes (literal chars) rather than quoting-for-literal, so they are
  unaffected. Record as a low `L-` note.
- **`)` at depth 0 in the operand** (`=~ a)`): matches bash's naive counting going
  negative — `saturating_sub` keeps depth at 0 and the `)` is a literal regex char;
  bash itself errors on the dangling `)` only when it unbalances a real grouping.
  Keep it simple (literal `)`); note any divergence if a probe shows one.
- **`=~` glued to text outside `[[ ]]`** (e.g. `echo a=~b`) is unaffected:
  `expect_regex` requires the standalone word `=~` AND `dbracket_depth > 0`.
