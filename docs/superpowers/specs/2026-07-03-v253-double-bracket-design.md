# v253 — `[[ … ]]` conditional expressions on the atom-command path (dormant, differential) — Design

**Status: APPROVED (2026-07-03).** Sixth Phase-C **Stage 2** "port a deferred
construct onto the atom-command path" iteration (after v248 funcdefs, v249
here-strings, v250 heredocs, v251 process substitution, v252 array literals).
Direction: `2026-06-30-phase-c-parser-driven-frontend-roadmap.md` (Stage 2) +
memory `huck-frontend-parser-driven-direction` / `huck-lexer-rearch-design`.

**Scope split (user decision):** v253 ports the `[[ … ]]` conditional-expression
grammar; the `=~` regex-match operator is DEFERRED to a follow-on v254 (it needs
a dedicated `Mode::Regex` for the regex operand). v253 recognizes `=~` and
returns a clean deferral so it can be flipped to full support in v254.

## 1. Goal & context

huck's front-end is being inverted so the lexer emits small atoms and the PARSER
(`crates/huck-syntax/src/parser.rs`) assembles words + structure — a DORMANT path
(gated by `command_atoms`, default `false`; production still uses the batch
Word-lexer + the `command.rs` oracle) that must produce ASTs byte-identical to
the oracle, gated by `new_seq` (atoms) vs `old_seq` (oracle) with `diff_cmd(s)`
asserting equality. Each Stage-2 iteration removes one construct family from the
atom path's deferred set. v253 ports **`[[ … ]]` extended-test compound
commands** (minus `=~`).

`[[ … ]]` is a COMPOUND COMMAND (`Command::DoubleBracket`), not a word part — so
this resembles v243 (if/while/for compounds) more than the recent word-level
ports. This is the CLEANEST possible port of the hardest-ranked remaining family:
**parser-only, no lexer change**, because the oracle applies no special
tokenization inside `[[ ]]` except the `=~` regex (deferred).

## 2. What already exists

- **AST (UNCHANGED):** `Command::DoubleBracket { expr: Box<TestExpr>,
  inline_assignments: Vec<Assignment> }` (`command.rs:619`). `TestExpr`
  (`command.rs:596`): `Unary { op: TestUnaryOp, operand: Word }`,
  `Binary { op: TestBinaryOp, lhs: Word, rhs: Word }`,
  `Regex { lhs: Word, pattern: Word }`, `Not(Box<TestExpr>)`,
  `And(Box, Box)`, `Or(Box, Box)`. All `pub`.
- **Oracle grammar (the reference to mirror, UNCHANGED):** `parse_double_bracket`
  / `parse_double_bracket_with_assigns` (`command.rs:2753`/`2759`) and the
  precedence cascade `parse_test_or` (`||`) → `parse_test_and` (`&&`) →
  `parse_test_not` (`!`, right-assoc) → `parse_test_primary` (`( expr )` grouping)
  → `parse_test_atom` (unary / lone-word / binary/regex). Helpers: `next_test_word`,
  `next_is_test_binary_operator` (`command.rs:2448`), `try_unary_op`
  (`command.rs:2486`), `try_binary_op`, `is_bang_word` (`command.rs:2517`),
  `is_test_expr_stop`, `skip_test_newlines`. These PULL from a `Lexer` via
  `iter.peek_kind()`/`iter.next_kind()` — the SAME API the atom parser uses —
  BUT read operands as a pre-lexed `TokenKind::Word`, which the atom stream does
  not have (it has `Lit`/expansion atoms). So the atom version must assemble
  operands via `parse_word_command`.
- **Errors (UNCHANGED):** `ParseError::EmptyDoubleBracket` (`[[ ]]`),
  `UnterminatedDoubleBracket` (`[[ a == b` — missing `]]` / EOF),
  `TestExprMissingOperand` (a `]]`/operator where an operand was expected).
- **Atom-path infra already present:** `Keyword::DoubleBracketOpen`/`Close` are in
  the atom keyword table (`parser.rs:1487`); `Command::DoubleBracket` is handled
  in the heredoc-attach walks (`parser.rs:774`, `2306`); the atom compound
  dispatch (`parse_command`, `parser.rs:~1891`) currently returns
  `UnsupportedCommand` for `[[`.
- **Tokenization fact (verified):** the production lexer's `dbracket_depth` is
  used ONLY to arm `expect_regex` on `=~` (`lexer.rs:2196-2198`); it does NOT gate
  assignment/array/procsub/`=` detection. So inside `[[ ]]` the oracle tokenizes
  operand words IDENTICALLY to command position (`[[ a=b ]]` → the single word
  `a=b`; array/procsub detection fires the same). Therefore the atom command
  scanner already emits the correct atoms inside `[[ ]]` — no new lexer mode.

## 3. Design — parser-only port

### 3.1 Dispatch hook

In the atom `parse_command` (`parser.rs:~1891`), add a `[[` arm alongside the
existing compound keywords: when `peek_leading_keyword` is
`Keyword::DoubleBracketOpen`, dispatch to `parse_double_bracket(iter,
Vec::new())`. (Currently `[[` falls to the `UnsupportedCommand` default.)

### 3.2 `parse_double_bracket` + the precedence cascade (new, in `parser.rs`)

Mirror the oracle's `parse_double_bracket_with_assigns` and cascade one-to-one,
adapting only operand reads:

- **`parse_double_bracket(iter, inline_assignments) -> Result<Command, ParseError>`**:
  consume `[[`; `skip_test_newlines`; if the next atom is the `]]` keyword →
  `EmptyDoubleBracket`; if EOF → `UnterminatedDoubleBracket`;
  `expr = parse_test_or(iter)?`; `skip_test_newlines`; consume `]]` (missing/EOF →
  `UnterminatedDoubleBracket`); return `Command::DoubleBracket { expr:
  Box::new(expr), inline_assignments }`.
- **`parse_test_or`**: `lhs = parse_test_and`; while next atom is `Op(Or)`: consume,
  `skip_test_newlines`, `rhs = parse_test_and`, fold `Or`. `skip_test_newlines`
  around iterations.
- **`parse_test_and`**: same shape on `Op(And)`, folding `And`.
- **`parse_test_not`**: if next atom is a bang word (`is_bang_word`) → consume,
  recurse `parse_test_not`, wrap `Not`; else `parse_test_primary`.
- **`parse_test_primary`**: if next atom is `Op(LParen)` → consume, `inner =
  parse_test_or`, expect `Op(RParen)` (EOF → `UnterminatedDoubleBracket`; other →
  `TestExprMissingOperand`), return `inner`; else `parse_test_atom`.
- **`parse_test_atom`** — the one adapted function:
  - EOF → `UnterminatedDoubleBracket`; a present terminator (`]]`/`)`, via
    `is_test_expr_stop`) → `EmptyDoubleBracket`.
  - Read the first operand as a Word via `parse_word_command(iter, false)` (guarded
    so it is only called when the peeked atom is a genuine word atom — never a
    separator/operator/`]]` — preserving progress).
  - If `try_unary_op(&first)` is `Some(op)` → read one more operand Word (a `]]`/
    operator/EOF here → `TestExprMissingOperand`/`UnterminatedDoubleBracket`) →
    `Unary { op, operand }`.
  - Else `first` is the lhs. If NOT `next_is_test_binary_operator` (next atom is
    `]]`/`)`/`Op(And)`/`Op(Or)`/EOF) → lone-word `Unary { op:
    TestUnaryOp::StringNonEmpty, operand: first }`.
  - Else consume the operator: a Word (`==`/`=`/`!=`/`-eq`/…) OR `Op(RedirIn)`/
    `Op(RedirOut)` for `<`/`>`. **If the operator Word is `=~` →
    `Err(UnsupportedCommand)` (v254 deferral) — return BEFORE reading the rhs so
    the regex operand is never pulled/mis-lexed.** Otherwise map to the
    `TestBinaryOp` (via `try_binary_op` / the `<`/`>` cases) and read the rhs Word →
    `Binary { op, lhs: first, rhs }`.

### 3.3 Reuse vs re-implement

- **Reuse from `command.rs` (make `pub(crate)`):** the pure classifiers
  `try_unary_op(&Word)`, `try_binary_op(&Word)`, `is_bang_word(&TokenKind)`,
  `is_test_expr_stop`-equivalent logic, and the `next_is_test_binary_operator`
  operator SET (its body pulls from the lexer, so port the peek but reuse the
  operator-text set / the `<`/`>` handling). `skip_test_newlines` similarly (a thin
  newline-drain — port to the atom cursor).
- **Re-implement (atom-native) in `parser.rs`:** `parse_double_bracket` + the five
  cascade functions + `parse_test_atom`'s operand reads (via `parse_word_command`)
  + `next_test_word`-equivalent.

### 3.4 Inline assignments

`FOO=hi [[ … ]]`: the atom simple-command path already collects leading
`NAME=value` assignment words. Where it currently proceeds to a simple command,
add the "peek `[[` after the collected assignments" check and route into
`parse_double_bracket(iter, assigns)` — mirroring the oracle's
`parse_command_or_pipeline` dispatch (`command.rs:1109-1111`).

## 4. Scope

**In scope** (byte-identical `diff_cmd`, or matching-error parity):

- Unary tests: every operator `try_unary_op` accepts (`-f`/`-d`/`-e`/`-r`/`-w`/`-x`/
  `-s`/`-z`/`-n`/`-L`/`-h`/`-b`/`-c`/`-p`/`-S`/`-g`/`-u`/`-k`/`-O`/`-G`/`-N`/`-t`/`-o`/
  `-v`/`-R`/…) — e.g. `[[ -f /etc/passwd ]]`.
- Binary tests: string (`==`, `=`, `!=`, `<`, `>`), arithmetic (`-eq`/`-ne`/`-lt`/
  `-le`/`-gt`/`-ge`), file (`-nt`/`-ot`/`-ef`) — `[[ $x == a* ]]` (glob RHS stays a
  pattern Word), `[[ 3 -eq 3 ]]`, `[[ a < b ]]`, `[[ a<b ]]`.
- Lone word `[[ x ]]` ≡ `-n x`; operands with expansions/quotes/globs
  (`[[ "$x" == "$y" ]]`, `[[ ${a[0]} -gt 0 ]]`).
- Logical `&&`/`||`/`!` with precedence + `( )` grouping + right-assoc `!` —
  `[[ -f a && -f b || ! -d c ]]`, `[[ ( a == b ) ]]`.
- Newlines around operators (`[[ a\n&&\nb ]]`), `[[\n expr \n]]`.
- Inline assignments: `FOO=hi [[ -n $FOO ]]`.
- Errors: `EmptyDoubleBracket` (`[[ ]]`), `UnterminatedDoubleBracket` (`[[ a == b`),
  `TestExprMissingOperand` (`[[ == b ]]`, `[[ -f ]]`). Split lexer-level rejects
  (where `old_seq` panics via `.expect("lex")`) from parser-level by observation,
  as existing error-parity tests do.
- `[[ ]]` as a pipeline/`&&`/`||` stage, negated (`! [[ … ]]`), with trailing
  redirects (`[[ … ]] >f`) — to the extent the atom compound-redirect wiring
  already covers other compounds (verify by observation; if a shape routes
  differently, note it, don't force it).

**Deferred (v254, per user).**

- `=~` regex match: `[[ x =~ re ]]` returns `UnsupportedCommand` on the atom path,
  tested via a deferral-parity assertion (the atom path errors where the oracle
  parses `TestExpr::Regex`). NOT a `diff_cmd`, and NOT a pinned divergence — it is
  an unported-family deferral flipped to full `diff_cmd` support in v254 (which
  adds `Mode::Regex` + the `scan_regex_operand` port). The parser MUST bail on the
  `=~` operator Word before pulling the rhs so the regex is never lexed.

## 5. Invariants

- Byte-identical: every in-scope `[[ … ]]` input parses to the SAME AST / same
  error on the atom path as the oracle. A well-formed in-scope divergence is a
  v253 BUG to fix (in the atom path), not to pin.
- Production untouched: `command_atoms` defaults `false`; the oracle's
  `parse_double_bracket` + `TestExpr` grammar + the lexer's `dbracket_depth` /
  `expect_regex` / `scan_regex_operand` are UNCHANGED (v253 only *reads* the
  reused classifiers via new `pub(crate)` visibility). `command.rs` behavior
  unchanged; `git diff main -- crates/huck-syntax/src/command.rs` limited to
  `pub(crate)` visibility widenings (no logic change). Engine-facing
  `Command::DoubleBracket` / `TestExpr` AST unchanged.
- No new lexer mode; `Mode::DoubleBracket` / `Mode::Regex` stay reserved (Regex
  arrives in v254).
- 0 warnings; every commit carries the `Co-Authored-By: Claude Opus 4.8 (1M
  context)` trailer; branch `v253-double-bracket`, not `main`.

## 6. Implementation staging (~4 tasks)

1. `[[`-dispatch in `parse_command` + `parse_double_bracket` + the `or/and/not/
   primary` cascade + `parse_test_atom` (unary / lone-word / binary, NON-`=~`);
   `pub(crate)` the reused classifiers. `diff_cmd` green for unary, binary
   (string+arith+file), lone-word.
2. Logical `&&`/`||`/`!` + `( )` grouping + newlines-in-expression, with a
   precedence corpus (`&&` binds tighter than `||`, right-assoc `!`, grouping
   overrides). `diff_cmd` green.
3. Inline assignments (`FOO=hi [[ … ]]`) + `[[ ]]` as pipeline/`&&`/`||`/negated
   stage + trailing-redirect observation + the `=~` deferral-parity assertion.
4. Full adversarial corpus (expansions/quotes/globs in operands, `[[`/`]]`/`!=`/`<`
   tokenization edges, lone-word-vs-binary boundary) + error parity (`Empty`/
   `Unterminated`/`MissingOperand`) + final gate.
