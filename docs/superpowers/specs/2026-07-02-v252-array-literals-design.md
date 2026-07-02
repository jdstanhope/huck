# v252 — Array literals (`name=(…)`) on the atom-command path (dormant, differential) — Design

**Status: APPROVED (2026-07-02).** Fifth Phase-C **Stage 2** "port a deferred
construct onto the atom-command path" iteration (after v248 funcdefs, v249
here-strings, v250 heredocs, v251 process substitution). Direction:
`2026-06-30-phase-c-parser-driven-frontend-roadmap.md` (Stage 2) + memory
`huck-frontend-parser-driven-direction` / `huck-lexer-rearch-design`.

## 1. Goal & context

huck's front-end is being inverted so the lexer emits small atoms and the
PARSER (`crates/huck-syntax/src/parser.rs`) assembles words + structure — a
DORMANT path (gated by `command_atoms`, default `false`; production still uses
the batch Word-lexer + `command.rs` oracle) that must produce ASTs byte-identical
to the oracle, gated by `new_seq` (atoms) vs `old_seq` (oracle) with `diff_cmd(s)`
asserting equality. Each Stage-2 iteration removes one construct family from the
atom path's deferred set. v252 ports **compound array-literal assignment
(`name=(…)`)**.

This is a MEDIUM-HARD, dedicated-mode port (closer to v250 heredocs than to the
v248/v249/v251 reuse ports): the reserved-but-unused `Mode::ArrayLiteral` becomes
real. The body is NOT a command sequence (so, unlike v251, it can't reuse
`Mode::CommandSub`) — it is a flat whitespace/newline/comment-separated list of
value-words, each optionally `[expr]=`-prefixed, terminated by `)`, with brace
expansion of bare elements.

## 2. What already exists

- **AST (UNCHANGED):** `WordPart::ArrayLiteral(Vec<ArrayLiteralElement>)`
  (`lexer.rs:359`); `ArrayLiteralElement { subscript: Option<Word>, value: Word }`
  (`lexer.rs:288`).
- **`Mode::ArrayLiteral`** (`lexer.rs:635`) — declared, NOT dispatched anywhere
  (reserved, like `HeredocBody` was pre-v250).
- **Production (the oracle, UNCHANGED):** the Word scanner's `=`/`+=`/`[sub]=`
  arms (`lexer.rs:2616`/`2635`/`2660`), after emitting the assignment prefix and
  setting `in_assignment_value`, do `skip_line_continuations` and — if the next
  char is `(` — consume it and call `scan_array_literal` (`lexer.rs:6099`),
  pushing `WordPart::ArrayLiteral(elements)` onto `self.parts`. So `name=(…)` is
  ONE word: `[Literal("name="), ArrayLiteral(...)]` (or `[AssignPrefix{…}, …]` for
  `+=`/`[sub]=`).
- **`scan_array_literal` grammar:** loop → `skip_array_literal_separators`
  (whitespace / newline / `\<NL>` / `#`-comment); `)` → done; EOF →
  `UnterminatedArrayLiteral`; optional `[expr]=` (via `scan_subscript`; a missing
  `=` after `]` → `ArrayLiteralMissingEquals`); a value via
  `scan_array_element_word` (stops ONLY at whitespace/`)` — so `|;&<>` are LITERAL
  in a value). A **subscripted** element keeps its single value; a **bare** element
  is `brace_expand_parts(value.0)` → N positional elements.
- **Atom-path assignment prefixes (v247 T4):** `try_scan_assign_prefix`
  (`lexer.rs:3521`) at word start emits `Lit "name="` (for `=`) or
  `AssignPrefix{…}` (for `+=`/`[sub]=`) and sets `in_assignment_value`. It does
  NOT currently look for a following `(` — the `(` falls to the operator arm as
  `Op(LParen)`, and the parser defers (`name=` is assignment-shaped + a bare
  `LParen` → `UnsupportedCommand`, the v251 catch-all).
- **Reusable atom machinery:** the shared expansion-opener emission (`$…`/`${`→
  `ParamOpen`/`$(`→`CmdSubOpen`/`$((`→`ArithOpen`/backtick→`BeginBacktick`/quote
  runs) in `scan_command_word_atom`; the `[…]` subscript atoms + subscript-operand
  mode (from `${a[i]}` / `a[i]=`); `brace_expand_parts`.

So v252 adds: `ArrayOpen`/`ArrayClose` atoms, array detection in
`try_scan_assign_prefix`, a `Mode::ArrayLiteral` scanner (dedicated value grammar
+ separators + subscript + close), and a parser `parse_array_literal`. No
`command.rs` / `scan_array_literal` / `scan_array_element_word` change.

## 3. Design

### 3.1 Lexer — detection + `Mode::ArrayLiteral`

New atoms: `TokenKind::ArrayOpen` (zero-width word-part signal, dual to
`CmdSubOpen`/`ProcSubOpen`; cursor left on `(`) and `TokenKind::ArrayClose`
(emitted by the mode on `)`).

**Detection (`try_scan_assign_prefix`):** in each of the `=`, `+=`, `[sub]=`,
`[sub]+=` branches, AFTER pushing the prefix atom and setting
`in_assignment_value`, do `skip_line_continuations` and peek `(`. If `(`, push a
`ArrayOpen` atom in the SAME `scan_step` (multi-token emit, like the `|&`
desugar), leaving the cursor ON the `(`. So the stream is
`[<prefix>, ArrayOpen, …body…]`. (Mirrors the production arms' inline
`if peek == '(' { … }`.)

**`Mode::ArrayLiteral` scanner (`scan_step_array_literal`):** the parser pushes
it on `ArrayOpen`. With a `body_started` flag (as `Mode::CommandSub` uses): when
`!body_started` the cursor is on `(` → consume it, flip `body_started`. When
`body_started`, emit ONE atom per `scan_step`, mirroring `scan_array_literal`'s
grammar over the cursor:
- inter-element separators (whitespace / newline / `\<NL>` continuation /
  `#`-comment run) → a `Blank` (or `Newline`) separator atom, so the parser can
  skip them uniformly (comments emit no content),
- `)` → `ArrayClose` (pop happens in the parser),
- at element start, `[` → emit the subscript-open atom (reuse the `${a[i]}` /
  `a[i]=` `[…]` machinery: `LBracket` + the subscript-operand mode) so the parser
  drives the `]`-matched subscript scan,
- otherwise value content → a `Lit { quoted:false }` run stopping ONLY at
  whitespace / `)` / a quote or `$`/backtick opener, plus the shared
  expansion-opener signals (`ParamOpen`/`CmdSubOpen`/`ArithOpen`/`BeginBacktick`)
  and quote runs — mirroring `scan_array_element_word`'s value grammar (NOT the
  command-word stop-chars: `|;&<>` are literal here),
- EOF before `)` → `LexError::UnterminatedArrayLiteral`.

The lexer NEVER scans ahead for the matching `)` — the parser owns the close;
obeys THE RULE.

### 3.2 Parser — `parse_array_literal` + word-assembler arm

- **`parse_array_literal(iter) -> Result<WordPart, ParseError>`** — push
  `Mode::ArrayLiteral { body_started: false }`; consume the opener (`next_kind`
  scans under the mode, consuming `(`). Loop:
  - skip separator atoms (`Blank`/`Newline`);
  - `ArrayClose` → `next_kind` (consume), pop mode, return;
  - subscript-open → parse the subscript `Word` from its atoms, require the
    following `=` (else pop + `ArrayLiteralMissingEquals`), set `subscript = Some`;
  - else assemble the value `Word` from value/expansion atoms, reusing
    `parse_word_command`'s part-handling (literal-coalescing + the shared
    `ParamOpen`/`CmdSubOpen`/`BeginBacktick`/`ArithOpen`/quote arms);
  - push the element(s): a **subscripted** element as-is (no brace expansion); a
    **bare** element via `brace_expand_parts(value.0)` → N positional
    `ArrayLiteralElement { subscript: None, value: Word(p) }`.
  - Pop the mode on EVERY exit path (incl. errors). Return
    `WordPart::ArrayLiteral(elements)`.
- **`parse_word_command`** — new arm: `Some(TokenKind::ArrayOpen)` →
  `iter.next_kind()?` (discard the signal), `flush_lit(...)`,
  `parts.push(parse_array_literal(iter)?)`. This glues the `ArrayLiteral` part
  after the already-assembled prefix part, yielding the same one-Word
  `[Literal("name="), ArrayLiteral(...)]` (or `[AssignPrefix{…}, ArrayLiteral(...)]`)
  shape as the oracle.
- `brace_expand_parts` — widen to `pub(crate)` if needed so the parser can call
  it (parser-depends-on-lexer).

## 4. Scope

**In scope** (byte-identical to the oracle via `diff_cmd`, or matching-error
parity):

- All four prefixes: `name=(…)`, `name+=(…)`, `name[sub]=(…)`, `name[sub]+=(…)`.
- Empty: `a=()`.
- Positional elements with quoting / expansions (`$x`/`${…}`/`$(…)`/`` `…` ``/
  `$((…))`) / globs / tilde.
- Subscripted `[i]=value` elements (single value, NO brace expansion — matches
  `scan_array_literal`).
- Brace-expanded bare elements: `a=({1..3})`, `a=(x{a,b}y)`.
- Literal metacharacters in values: `a=(a|b c;d e<f)` (each is ONE element's
  text — `|;&<>` are literal).
- Separators: multiple spaces, newlines inside the literal (`a=(\n x \n y \n)`),
  `\<NL>` continuations (`arr=\<NL>(…)` and between elements), `#`-comments.
- Nested expansions in values (`a=($(echo x) ${y} \`z\`)`).
- Error parity: `UnterminatedArrayLiteral` (EOF before `)`),
  `ArrayLiteralMissingEquals` (`[i]` without `=`). Split lexer-level rejects
  (where `old_seq` panics via `.expect("lex")`) from parser-level by observation,
  as existing error-parity tests do.

**Out of scope / deferred.**

- Array literals in `declare`/`local`/`export`/`readonly` ARG position (e.g.
  `declare -a x=(1 2)`) IF they route through a different path (`DeclArg`
  pre-parse) than the command-word assignment path — verify during
  implementation; if they route through the SAME word path, they come along for
  free (no extra work). If different, defer + note.
- The live flip (`command_atoms` stays `false`); other deferred families
  (`[[ … ]]`, arith command, coproc, `$[ ]`); the v250 live-flip carry-forwards.

## 5. Invariants

- Byte-identical: every in-scope array-literal input parses to the SAME AST / same
  error on the atom path as the oracle. A well-formed in-scope divergence is a
  v252 BUG to fix, not to pin.
- Production untouched: `command_atoms` defaults `false`; `scan_array_literal`,
  `scan_array_element_word`, `scan_subscript`, `skip_array_literal_separators`,
  the production `=`/`+=`/`[sub]=` Word-scanner arms, `command.rs`, `process_line`
  all UNCHANGED. Engine-facing `WordPart::ArrayLiteral` / `ArrayLiteralElement`
  AST unchanged.
- The scalar-assignment atom path (`x=y`, `x+=y`, `x[i]=y`) and non-`(`
  assignment values are UNAFFECTED — the `ArrayOpen` emission fires only when a
  `(` immediately follows the prefix.
- 0 warnings; every commit carries the `Co-Authored-By: Claude Opus 4.8 (1M
  context)` trailer; branch `v252-array-literals`, not `main`.

## 6. Implementation staging (~4 tasks)

1. `ArrayOpen`/`ArrayClose` atoms; array detection in `try_scan_assign_prefix`'s
   four branches (emit `ArrayOpen` when `(` follows the prefix); `Mode::ArrayLiteral`
   scanner emitting value/separator/close atoms for the POSITIONAL (non-subscript)
   value grammar + `UnterminatedArrayLiteral`. DORMANT — parser still defers, gate
   unchanged; verify at the atom-stream level.
2. Parser `parse_array_literal` + the `parse_word_command` `ArrayOpen` arm, for
   positional + brace-expanded bare elements; remove the deferral. `diff_cmd`
   green for `name=(…)`/`+=`, empty, quoting/expansion values, brace expansion,
   literal `|;&<>`.
3. Subscripted `[i]=` elements: the subscript-open atom/scan in the mode +
   `parse_array_literal`'s subscript branch + `ArrayLiteralMissingEquals`.
   `name[sub]=(…)` prefix path. `diff_cmd` green for subscripted elements + mixed.
4. Full corpus: separators (spaces/newlines/`\<NL>`/comments), nested expansions,
   `declare`/`local` observation (include if same path, else defer + note), error
   parity, adversarial. Full `huck-syntax` lib + doctests green, 0 warnings.
