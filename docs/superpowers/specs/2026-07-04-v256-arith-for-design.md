# v256 — C-style `for (( … )); do … done` on the atom-command path

**Status:** design approved (2026-07-04)
**Arc:** Phase C, Stage 2 — porting deferred construct families onto the dormant
atom-command parser (`command_atoms`, default `false`), one family per iteration,
byte-identical to the `command.rs` oracle, marching toward the finale (flip
`command_atoms` live + delete the forward-scanning production scanners).

## Summary

Port the C-style arithmetic `for` loop `for (( init; cond; step )); do … done`
(bash's `Command::ArithFor`) onto the atom-command parser. This closes the
arith-command family (v255 did the standalone `(( expr ))`; v256 does the
`for ((…))` header). Today the atom `parse_for` DEFERS the moment it sees `((`
after `for` (`parser.rs:2716-2737` → `UnsupportedCommand`); v256 replaces that
deferral with a real parse.

**Dormant + differential + parse-time only.** `command_atoms` stays `false`; the
production path still uses the Word-lexer's pre-scanned `ArithBlock` →
`Command::ArithFor`, untouched. The atom path builds the *same*
`Command::ArithFor(Box<ArithForClause { init, cond, step, body }>)` AST. Runtime
evaluation is existing production code, unchanged.

**Scope (confirmed):** the full `for (( init; cond; step )); do … done`, including
empty/partial sections (`((;;))`, `((i=0;;))`), embedded expansions in any section,
`do`/`done` body composition (pipeline/list/redirect/nested-in-compound), and the
malformed/unterminated error parity. Nothing new deferred. Runtime `ArithFor`
evaluation untouched.

## Background — the oracle vs THE RULE

The oracle Word-lexer scans a COMPLETE `(( … ))` header (after `for`) as a single
pre-scanned `TokenKind::ArithBlock(String)` (a forward scan to the matching `))`).
`parse_for_command` (command.rs:1487) dispatches `ArithBlock → ArithFor` via
`parse_arith_for_clause` (command.rs:1460), which:
1. `split_top_level_semi(header_text)` (command.rs:1396) — splits on `;` at
   **paren**-depth 0 (purely `(`/`)` counting; quotes/braces/backticks NOT tracked);
2. requires exactly 3 sections, else `Err(ParseError::ArithForHeader("expected 3
   sections separated by `;`, got {N}"))`;
3. per section: `trim()` → empty ⇒ `None`, else `arith_string_to_word(trimmed)` ⇒
   `Some(Word)`;
4. skips `;`/newline separators, `expect Do`, body via `parse_compound_section`
   until `Done`, `expect Done`.

That forward scan (and `arith_string_to_word`, which internally calls the
forward-scanning production scanners the finale deletes) is exactly what THE RULE
forbids on the atom path — reusing the oracle's string splitter would be a
live-flip blocker (the same reason the v243 `arith_string_to_word` approach was
rejected). So v256 is **atom-native**: tokenize the header as arith via
`Mode::Arith`, emit the section-separating `;` as an atom, and assemble the section
`Word`s in the parser.

## Chosen approach — atom-native section split (rejected alternative: raw-string)

**Rejected (A′):** capture the raw header substring (via atom span offsets) and call
the oracle's `parse_arith_for_header`. Byte-identical by construction, but routes
sections through `arith_string_to_word` → the to-be-deleted scanners → a live-flip
blocker. Not chosen.

**Chosen (B):** reuse `Mode::Arith` (v255/v246) to tokenize the header as arith, add
a `for_header` flag that emits a depth-0 `;` as an `ArithSemi` separator atom (using
the lexer's authoritative `paren_depth`, so the parser never re-derives depth), and
assemble three section `Word`s in the parser (trim + empty-⇒-`None`). Atom-native,
finale-compatible, and keeps THE RULE's division (lexer emits atoms incl. the
separator; parser assembles words AND structure).

**Unambiguous — no subshell bail.** After `for`, a glued `((` is *always* an
arith-for header (bash allows no subshell there). Unlike v255's command-position
`((`, there is **no mark/rewind and no bail-to-subshell**: the header either closes
with `))` (→ `ArithFor`) or it doesn't (→ an error matching the oracle).

## Architecture

**Files:**
- `crates/huck-syntax/src/lexer.rs` — add `for_header: bool` to `Mode::Arith`; add
  `TokenKind::ArithSemi`; add the depth-0-`;` branch in `scan_step_arith`.
- `crates/huck-syntax/src/parser.rs` — `parse_arith_for_clause` +
  `parse_arith_for_body` + `trim_section` + the dispatch replacement in `parse_for`
  + tests.
- `crates/huck-syntax/src/command.rs` — UNTOUCHED (empty diff).

### Lexer

- `Mode::Arith { paren_depth: u32, in_dquote: bool, body_started: bool, for_header: bool }`.
  The two existing construction sites (v246 `$((` in `parse_arith_expansion`, v255
  `((` in `parse_arith_command`) set `for_header: false` — mechanical, behavior
  byte-unchanged.
- `TokenKind::ArithSemi` — zero-width section separator.
- In `scan_step_arith`'s body loop, a `;` while `for_header && depth == 0` flushes
  any pending `Lit` first (mirroring the existing depth-0 `)` arm at lexer.rs:2006),
  then consumes the `;` and emits `ArithSemi`. When `!for_header`, `;` stays an
  ordinary literal char (so `$((`/`((` are byte-unchanged). Nested `;` (depth > 0)
  stays literal. Quotes/`$(…)`/`` `…` ``/`${…}` are sub-modes, so a `;` inside them
  is never seen here (the known rare-edge divergence below).

### Parser

`parse_arith_for_clause` (replaces the parser.rs:2716-2737 deferral inside
`parse_for`):

```
consume the two Op(LParen)                              // like v255 parse_arith_command
push Mode::Arith { paren_depth:0, in_dquote:false, body_started:true, for_header:true }
let sections = parse_arith_for_body(iter)?              // Vec<Word>, split on ArithSemi
pop_mode                                                // on ALL paths
if sections.len() != 3 {
    return Err(ParseError::ArithForHeader(
        format!("expected 3 sections separated by `;`, got {}", sections.len())))
}
let init = trim_section(&sections[0]);   // Option<Word>
let cond = trim_section(&sections[1]);
let step = trim_section(&sections[2]);
skip Blank / Op(Semi) / Newline separators             // header → do
expect Do (UnterminatedLoop)
let body = parse_compound_section(iter, &[Keyword::Done], UnterminatedLoop)?
expect Done (UnterminatedLoop)
Ok(Command::ArithFor(Box::new(ArithForClause { init, cond, step, body })))
```

- **`parse_arith_for_body`** mirrors v255's `parse_arith_body` part-assembly
  (`Lit`/`DollarName`/`ParamOpen`/`CmdSubOpen`/`BeginBacktick`/nested `ArithOpen`,
  all `quoted:true`), accumulating into a current section `Word`. On `ArithSemi` →
  push the current section, start a fresh one. On `ArithClose` → push the final
  section, return `Ok(sections)`. On EOF/unterminated → the oracle-matching error
  (the v184 "unterminated `for ((`" path — probed at plan time). There is no
  `Bail`→subshell arm (no subshell alternative after `for`).
- **`trim_section(&Word) -> Option<Word>`:** trim leading whitespace from the first
  part's `Lit` text and trailing whitespace from the last part's `Lit` text (drop
  now-empty `Lit`s); no parts ⇒ `None`, else `Some`. Reproduces the oracle's
  `s.trim()` + empty-⇒-`None`. Since v246 proved the atom arith-`Word` equals
  `arith_string_to_word` for the same expression, a trimmed atom section equals
  `arith_string_to_word(trim(section_string))` part-for-part.

**Dispatch:** in `parse_for`, after `expect For` + `skip_newlines`, a glued `((`
(peek `Op(LParen)` && peek2 `Op(LParen)`) → `parse_arith_for_clause`. The legacy
`ArithBlock(..)` arm stays (never hit on the atom path; keeps `parse_for` total).

### Count-error parity

`sections.len()` = `ArithSemi` count + 1, matching `split_top_level_semi`'s count
exactly: `((a;b;c;d))` → 4 ("got 4"), `((a))` → 1 ("got 1"), `((;;))` → 3 (all
`None`). The formatted message string matches, so `diff_err` matches.

### Progress / OOM safety

`parse_arith_for_body` makes progress every pull (consumes an atom, or returns on
`ArithClose`/error). No mark/rewind, no loop.

## Differential corpus

All `diff_cmd` (atom `new_seq` AST == oracle `old_seq` AST) unless marked `diff_err`
(both paths return the SAME `Result`). Every value is probed against the oracle
before the plan is finalized (as in v255).

**Well-formed → `Command::ArithFor`:**
- `for ((i=0;i<3;i++)); do echo $i; done`
- `for ((;;)) do :; done` — all sections empty (all `None`)
- `for (( i = 0 ; i < n ; i++ )); do x; done` — spaces → trimmed
- `for ((i=0,j=0; i<3; i++,j++)); do :; done` — comma operator is literal, not a separator
- `for ((i=$x; i<${n}; i++)); do :; done` — embedded expansions
- `for ((i=(1+2); i<9; i++)); do :; done` — inner grouping parens (depth-tracked)
- `for\n((;;)); do :; done` — newline before header
- `for ((;;)); do break; done | cat` and `for ((;;)); do :; done >out` — composition/redirect wrap

**Error parity (`diff_err`):**
- `for ((a;b;c;d)); do :; done` → `ArithForHeader` "got 4"
- `for ((a)); do :; done` → `ArithForHeader` "got 1"
- `for ((i=0;i<3;i++)` — unterminated header (→ the oracle's `for ((` fallback error; probe to pin exactly)
- missing `do` / `done` → `UnterminatedLoop`

## Known divergence pins

A `;` inside a quote / backtick / `${…}` in the header (`for (( "a;b"; ; ))`): the
oracle's `split_top_level_semi` ignores sub-structure and splits inside it (→ 4
sections → error), while the atom path treats it as a sub-mode so the inner `;` does
not split. Rare and degenerate (quotes in an arith header). Pin whatever the actual
both-paths behavior is (likely oracle-errors vs atom-parses) with a test, and record
it as a live-flip carry-forward — do not contort the design. Any other edge surfaced
by plan-time probing gets the same treatment.

## Testing & gates

- Differential harness in `parser.rs mod tests`: `diff_cmd` / `diff_err`.
- `command.rs` diff-vs-main stays EMPTY.
- `lexer.rs` diff limited to the `for_header` field + the `ArithSemi` branch; the
  whole existing suite (every `$((`/`((`/arith test) is the regression gate for
  byte-unchanged `!for_header` behavior.
- Both `command_atoms` sites (lexer.rs:811, lexer.rs:4167) stay `false`.
- `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` green.
- `cargo build -p huck-syntax` → 0 warnings.
- Progress/OOM: `parse_arith_for_body` verified non-looping.

## Task decomposition

- **T1 — lexer:** `for_header` flag on `Mode::Arith` (existing sites → `false`) +
  `TokenKind::ArithSemi` + the depth-0-`;` branch in `scan_step_arith`. A focused
  test that a `for_header` arith body emits `ArithSemi` at top-level `;` while
  `$((`/`((` remain byte-unchanged (the existing arith suite is the regression net).
- **T2 — parser core:** `parse_arith_for_clause` + `parse_arith_for_body` +
  `trim_section` + the `parse_for` dispatch replacement. Corpus: the well-formed
  cases + the section-count-error (`ArithForHeader`) cases.
- **T3 — composition + edges + flips:** pipeline/list/redirect/nested-in-compound
  composition + unterminated/missing-`do`/`done` error parity + the divergence pins +
  flip the pre-existing `for ((...))` deferral assertions (`cmd_compound_deferred_still`
  and the other deferral tests listing `for ((...))`).

## Live-flip carry-forwards

Anticipated: the quote/backtick/`${…}`-containing-`;` header split divergence
(above). Any additional divergence found during implementation is pinned with a test
and recorded in the ledger, per the v248–v255 convention. This iteration closes the
arith-command family (standalone `(( ))` v255 + C-for `for (( ))` v256).
