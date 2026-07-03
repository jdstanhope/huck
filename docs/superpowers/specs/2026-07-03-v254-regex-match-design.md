# v254 — `=~` regex match inside `[[ … ]]` on the atom-command path

**Status:** design approved 2026-07-03. The deferred half of v253 (`[[ … ]]`
shipped `=~`-deferred; this closes it).

## Goal

Port the `=~` regex-match operator inside `[[ … ]]` onto huck's DORMANT
atom-command parser, byte-identical to the `command.rs` oracle, using the
reserved-but-unused `Mode::Regex`. Parse-time only: the atom path must build
the same `TestExpr::Regex { lhs, pattern: Word }` AST as the oracle; runtime
regex expansion/matching is production code and is untouched.

## Background: why `=~` needs its own lexer mode

After `=~` inside `[[ … ]]`, the pattern operand has tokenization rules unlike
any normal word — this is exactly why v253 deferred it. Per the oracle's
`scan_regex_operand` (lexer.rs:4295):

- `|` `<` `>` `;` `&` are **literal** regex metacharacters (in a normal word
  they are operators/separators that would split or stop the word).
- Whitespace at **paren-depth 0 terminates** the operand, but whitespace
  **inside `( … )` is kept** — the scan tracks `()` depth
  (`[[ $x =~ (a b)c ]]` → pattern `(a b)c`).
- `$…` (parameter/`$(…)` command-sub/`$((…))` arith), `` `…` `` backtick,
  `'…'` / `"…"` quoting all expand/quote normally, producing a **multi-part**
  pattern `Word`.
- `\<newline>` is a line-continuation; `\x` keeps both the backslash and the
  char; no brace expansion, no tilde, no glob.

The production does this with a single forward-scanning function that reads
ahead to the terminating whitespace and recurses into nested expansions —
exactly what THE RULE forbids on the atom path (lexer emits small atoms, never
scans ahead for a delimiter; parser owns delimiter-matching/recursion). The
atom command scanner would otherwise mis-tokenize the operand (turning
`|<>;&` into operators), which is why v253 returned `UnsupportedCommand`.

## Architecture (approach A1: atom-native `Mode::Regex`)

Dormant, differential, parser-driven. `command_atoms` stays `false`; the
production/oracle path already does `=~` and is untouched.

Control flow — the parser drives the mode push/pop:

1. `parse_test_atom` reads `lhs`, then the operator word; when it is `=~`,
   instead of today's `Err(UnsupportedCommand)`, it **pushes `Mode::Regex`**
   onto the lexer mode stack as its first action (before any peek, so the
   operand is never tokenized in command mode).
2. The lexer, in `Mode::Regex`, tokenizes the pattern operand under regex
   rules — emitting **small atoms**: literal runs (including `|<>;&` and
   depth-tracked `()`) as `Lit`, and on `$`/`` ` ``/`"`/`'` the **same
   expansion-opener signals** `scan_command_word_atom` already emits
   (`DollarName`/`ParamOpen`/`ArithOpen`/`CmdSubOpen`/`BeginBacktick`/dquote/
   squote), so `${…}`/`$(…)`/`$((…))`/backtick/dquote recurse through the
   existing v241–v246 sub-modes.
3. At the terminating **depth-0 whitespace** (or EOF), the lexer **pops back
   to command mode** and emits a zero-width `RegexEnd` boundary atom, leaving
   the whitespace for command mode to tokenize normally.
4. The parser assembles the emitted atoms into `pattern: Word` (reusing the
   `parse_word_command`-style part-assembly — no special parser logic,
   because the lexer already emitted the metacharacters as `Lit` rather than
   `Op`) and builds `TestExpr::Regex { lhs, pattern }`.

The lexer owns the regex-specific *tokenization* (what is literal, where the
operand ends) as a running incremental scan; the parser owns the *when* (it
alone knows `=~` introduces a regex) and the *assembly/recursion*.
RULE-compliant, and it lets the finale delete the forward-scanning
`scan_dollar_expansion` (which `scan_regex_operand` depends on) cleanly.

**Files touched:** `lexer.rs` (extend the reserved `Mode::Regex` unit variant
to `Regex { paren_depth: u32 }`, add `scan_step_regex`, add the `RegexEnd`
atom) and `parser.rs` (the `=~` arm + pattern assembly + tests). `command.rs`
is expected to be untouched — `TestExpr::Regex` and `word_literal_text` are
already `pub`; confirm during planning (a `pub(crate)` visibility widening is
the only permitted `command.rs` change if some helper is needed).

## Lexer: `Mode::Regex` scanning rules (`scan_step_regex`)

Modeled on `scan_command_word_atom` for the `$`/quote classification, with
regex-specific literal/terminator rules. Emits atoms char-by-char, maintaining
the running `paren_depth`. Semantics mirror `scan_regex_operand` exactly.

**Leading whitespace (operand not yet started):** consume and discard
spaces/tabs/newlines between `=~` and the first operand char (matches the
oracle's `expect_regex` whitespace-skip). A `\<newline>` continuation here is
also consumed. Emit nothing.

**Once the operand has content, per char:**

- `(` → `paren_depth += 1`, append to the current literal run.
- `)` → `paren_depth = paren_depth.saturating_sub(1)`, append to the run.
- **depth-0 whitespace** (space/tab/newline) → **terminator**: flush the
  pending literal, pop `Mode::Regex`, emit a zero-width `RegexEnd`, and leave
  the whitespace for command mode (which emits its usual `Blank`/`Newline`).
  At `paren_depth > 0`, whitespace is **literal** (appended to the run).
- `|` `<` `>` `;` `&` and every other regex char → **literal** (appended). Not
  operators/separators inside `Mode::Regex`, so the parser sees `Lit`, not
  `Op`.
- `$` → flush, then the same `$`-classification the shared scanner uses:
  `${`→`ParamOpen`, `$(`→`CmdSubOpen`, `$((`→`ArithOpen`,
  `$name`/`$1`/`$@`→`DollarName`, lone `$`→`DollarLit`. Cursor left per each
  signal's convention; the parser recurses via the existing sub-mode.
- `` ` `` → flush, emit the backtick opener (`BeginBacktick`) → `Mode::Backtick`.
- `'…'` → flush, emit the single-quoted run as a `quoted: true` literal atom.
- `"…"` → flush, emit the dquote opener → `Mode::DoubleQuote`.
- `\<newline>` → line-continuation: consume both, emit nothing.
- `\x` (any other char) → literal `\x` (backslash **and** the char kept, per
  `scan_regex_operand`) — so `a\ b` is one pattern with a literal
  backslash-space; the escaped space does **not** terminate.
- EOF → flush, pop mode, emit `RegexEnd`. (The enclosing `[[` then hits EOF
  with no `]]` → `UnterminatedDoubleBracket`, matching the oracle.)

**Terminator/pop mechanism:** the explicit zero-width `RegexEnd` atom is the
boundary — the lexer pops `Mode::Regex` and emits `RegexEnd` at the
terminating whitespace/EOF (cursor still *on* the whitespace, so command mode
re-consumes it as a `Blank`). The parser's assembly loop pulls pattern atoms
until `RegexEnd`. `RegexEnd` is the only new atom; `Mode::Regex`-emitted
`(`/`)`/metacharacters are ordinary `Lit`s.

**Consequence:** because only depth-0 whitespace terminates, `[[ $x =~ ]]`
scans `]]` *as the pattern* (no whitespace before EOF) → then no closing `]]`
→ `UnterminatedDoubleBracket` — exactly the oracle's behavior.

## Parser: the `=~` arm + pattern assembly (`parse_regex_operand`)

Today's `parse_test_atom` operator match has
`"=~" => Err(ParseError::UnsupportedCommand)`. Replace it with a
`parse_regex_operand` call that produces the pattern `Word`, then return
`Ok(TestExpr::Regex { lhs, pattern })`.

**The `=~` arm (order matters — push before any peek):**

1. The `=~` operator word has just been consumed by `parse_word_command`.
   From v253, `next_is_test_binary_operator_atom`'s `peek2` already buffered
   exactly **one** boundary atom after `=~` (the `Blank`/`Newline` separating
   `=~` from the operand) — and only that one, never into the operand. So the
   operand is still untokenized.
2. `iter.push_mode(Mode::Regex { paren_depth: 0 })` as the first action.
3. Skip any already-buffered leading `Blank`/`Newline` atom(s) the separator
   peek2 consumed (mode-independent whitespace); the lexer's own
   leading-whitespace skip handles the rest from the cursor. (Mirrors v253's
   `skip_test_blanks`, extended to also drop a leading `Newline` here since
   the oracle skips a raw newline between `=~` and the operand.)
4. Assemble the pattern `Word` by pulling atoms until `RegexEnd`: literal runs
   → `WordPart::Literal`, and the expansion openers recurse through the
   existing helpers (`parse_param_expansion`/`parse_command_sub`/`parse_arith`/
   `parse_backtick`/`parse_dquote`) exactly as `parse_word_command` already
   does. Because the lexer emitted metacharacters as `Lit` (not `Op`), this is
   the same part-assembly `parse_word_command` uses — factor the shared loop so
   the regex assembly reuses it, stopping on `RegexEnd` instead of the
   command-context stops.
5. On `RegexEnd`, the mode is already popped (the lexer popped it when it
   emitted `RegexEnd`); consume the `RegexEnd` atom and return
   `TestExpr::Regex { lhs, pattern }`.

**Empty-pattern / EOF:** if the first atom after the leading-whitespace skip
is `RegexEnd` (empty operand — only at EOF, since `]]` etc. scan *as* pattern
text), the pattern is an empty `Word([])`; the enclosing `[[` then finds no
`]]` → `UnterminatedDoubleBracket`, matching the oracle.

**Precedence/nesting:** the `=~` arm sits inside `parse_test_atom`, so a regex
is a primary — `[[ -f a && $x =~ b|c ]]`, `[[ $x =~ a || $y =~ b ]]`,
`[[ ( $x =~ b ) ]]`, `[[ ! $x =~ b ]]` compose through the existing cascade
with no extra work.

**No mark/rewind:** the whole flow is forward-only (push mode → pull atoms →
`RegexEnd`), avoiding the v248 mark-after-peek hazard entirely.

## Differential corpus (each becomes `diff_cmd` / `diff_err`)

- Plain + anchors: `[[ $x =~ ^abc$ ]]`, `[[ $x =~ [0-9]+ ]]`, `[[ $x =~ a.c ]]`
- Metachars literal: `[[ $x =~ a|b ]]`, `[[ $x =~ a<b>c ]]`, `[[ $x =~ a;b ]]`,
  `[[ $x =~ a&b ]]`, `[[ $x =~ a*b? ]]`
- Paren-depth whitespace: `[[ $x =~ (a b) ]]`, `[[ $x =~ ((a) (b))+ ]]`,
  `[[ $x =~ (a b)c ]]`
- Expansions: `[[ $x =~ $p ]]`, `[[ $x =~ ${p}x ]]`, `[[ $x =~ ${a[0]} ]]`,
  `[[ $x =~ $(cmd) ]]`, `[[ $x =~ $((1+1)) ]]`, `` [[ $x =~ `cmd` ]] ``,
  `[[ $x =~ a$b|c$(d) ]]`
- Quoting: `[[ $x =~ "a b" ]]`, `[[ $x =~ 'a.b' ]]`, `[[ $x =~ x"$y"z ]]`,
  `[[ $x =~ \. ]]`, `[[ $x =~ a\ b ]]`
- Continuation: `[[ $x =~ a\<NL>b ]]`, `[[ $x =~ \<NL>  foo ]]`
- Terminator edges: `[[ $x =~ ]]` (→ Unterminated, pattern `]]`),
  `[[ $x =~ foo` (EOF → Unterminated), `[[ $x =~ a]] ]]` (pattern `a]]`, then
  `]]` closes)
- Composition: `[[ -f a && $x =~ b|c ]]`, `[[ $x =~ a || $y =~ b ]]`,
  `[[ ( $x =~ b ) ]]`, `[[ ! $x =~ b ]]`
- v253 carry-forwards stay green: `[[ a =~$x ]]` (glued → not recognized as
  `=~` operator → lone-word → Unterminated, both); `[[ a =~ $x ]]` (spaced →
  now a real regex — flip its `diff_unsupported` to `diff_cmd`)

## Scope

- **In:** `=~` inside `[[ … ]]` on the atom path, full `scan_regex_operand`
  parity (literal metachars, paren-depth whitespace, `$`/backtick/quote
  expansions, escapes, continuations).
- **Out (unchanged):** runtime regex matching/expansion (production code,
  already works); `=~` outside `[[ ]]`; the pre-existing live-flip
  carry-forwards from earlier iterations.
- **Flip:** v253's `atoms_double_bracket_regex_deferred` — its
  `diff_unsupported`/`UnsupportedCommand` assertions become `diff_cmd` (the
  deferral is closed). The v254 equivalent of v253 flipping the T1
  inline-assignment pin.

## Testing & gate

Every in-scope input is a `diff_cmd(s)` asserting `new_seq(s)` (atom path) ==
`old_seq(s)` (oracle); two-`Err` cases use `diff_err`. On any divergence, fix
the atom path — never the oracle. TDD per task. Gate = full lib suite +
doctests green, 0 warnings, `command.rs` untouched (or visibility-only),
`command_atoms` false at both sites. Box constraint: only
`cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (a
`--workspace`/multi-threaded run — or a non-progress infinite loop — OOM-kills
the session).

## Task decomposition (for the plan)

~3 tasks:

1. **T1** — `Mode::Regex { paren_depth }` + `scan_step_regex` + the `RegexEnd`
   atom + the parser `=~` arm/assembly, with the core corpus (literals,
   metachars, paren-depth whitespace, plain `$`/`$(…)`/`$((…))`/backtick
   expansions).
2. **T2** — quoting (`"…"`/`'…'`/mixed) + escapes (`\.`, `\ `) + line
   continuations + terminator edges (`[[ $x =~ ]]`, EOF, `a]] ]]`).
3. **T3** — composition (logical/grouping/negation nesting) + flip the v253
   deferral test + adversarial corpus + final gate.

## Non-goals / notes

- No `bash-divergences.md` change (dormant port, no user-visible divergence —
  same as v248–v253).
- New live-flip carry-forwards: none expected (all expansion families the
  regex operand can nest are already ported).
- THE RULE preserved: the lexer emits small atoms and tracks only a running
  incremental `paren_depth` (like v246's arith mode) — it never scans ahead
  for a matching delimiter; the parser owns the recursion into `$(…)`/`${…}`.
