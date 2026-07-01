# v246 — `$(( … ))` arithmetic-expansion lexer mode (design)

**Date:** 2026-07-01
**Status:** approved (brainstorm), pending implementation plan
**Arc:** Phase C, Stage 1 — the parser-driven front-end re-architecture. Prior
iterations: v240 (mode stack + mark/rewind), v241 (`${…}` ParamExpansion), v242
(flat command list), v243 (compounds), v244 (`$( … )` CommandSub), v245
(`` `…` `` Backtick + arbitrary-depth nesting). This is the next word-level
divergent mode.

## Goal

Invert `$(( … ))` arithmetic expansion into the DORMANT parser-driven
lexer/parser, following the v244/v245 template: the LEXER emits small atoms and
never scans ahead for a matching delimiter; the PARSER owns delimiter-matching,
recursion, and assembly of the `WordPart::Arith` AST. Differential-tested
byte-identical against the production lexer/parser (the ORACLE). All new code is
dormant (reached only by the differential harness and the `${…}` operand path)
and adds no production behavior change.

## Background: how production handles `$(( … ))` (what we are replacing)

The batch lexer builds `WordPart::Arith { body: Word, quoted }` in TWO passes,
both of which are forward-scanning and are exactly what this re-architecture
deletes:

1. `scan_arith_body` (lexer.rs) — after the opening `$((` is consumed, scans
   FORWARD tracking raw paren depth (start 1; `(`→+1, `)`→ if depth 1 the next
   char must be `)` to close `))`, else depth−1) until the matching `))`, and
   returns the inner text as a raw `String`.
2. `arith_string_to_word` — RE-LEXES that raw string into the body `Word`,
   recognizing `$var`/`${…}`/`$(…)`/`$((…))`/backticks inside.

The `$(( vs $( (` disambiguation is also forward-scanning: `scan_dollar_expansion`
clones the cursor, tries `scan_arith_body`, and on failure rewinds the clone and
reparses as a command substitution whose body begins with a subshell (`$( (…) )`).
See `scan_dollar_expansion` (lexer.rs ~3224) and `scan_arith_body` (~3523).

The key AST fact: **`WordPart::Arith { body: Word, quoted }`** — the arith body
is already a `Word` (a `Vec<WordPart>`), not a raw string. At runtime the engine
expands that `Word` to a string and hands the string to `arith.rs` to parse and
evaluate. Example: `echo $((3 * $((5*10))))` yields body `Word` ≈
`[Literal("3 * "), Arith { body: [Literal("5*10")] }]`; the inner expands to
`50`, then `arith.rs` evaluates `3 * 50`.

## Core rule (non-negotiable)

The lexer emits small atoms and NEVER scans ahead for a matching delimiter. It
moves forward one atom at a time with at most a single bounded char peek. The
PARSER owns all recursion (via mode push/pop) and owns the `$(( vs $( (`
disambiguation (via v240 mark/rewind). No `scan_arith_body`, no
`arith_string_to_word`, no cursor-clone-and-scan-ahead. The production scanners
stay untouched; the new mode is parallel and dormant.

## Scope

**In scope:** the word-level `$(( … ))` arithmetic expansion mode, including:
- literal expression body content;
- grouping parens inside the expression (`$(( ((1+2))*3 ))`);
- embedded expansions in the body (`$var`, `${…}`, `$(…)`, `` `…` ``, nested `$((…))`);
- quoted (inside `"…"`) vs unquoted context;
- the `$( (…) )` wrinkle (a `$((` that is really a command-sub of a glued
  subshell), handled FULLY via mark/rewind;
- unterminated-input error parity (`LexError::UnterminatedArith`);
- wiring into the `${…}` operand path so `${x:-$((1+1))}` parses.

**Out of scope (deferred):**
- the legacy `$[ … ]` synonym (shares `arith_string_to_word`; a later follow-on);
- the command-form `(( … ))` and arith-`for` headers (gated on the Stage-2
  Command-mode-emits-atoms rewrite — Command mode pre-disambiguates `((` today);
- live-wiring into the production Command-mode word scanner (that is Stage 2).

## Architecture

### New atoms (`TokenKind`, `#[non_exhaustive]`)

- `ArithOpen` — the `$((` opener. Dual role (like `CmdSubOpen`/`BeginBacktick`):
  a signal in an operand mode (operand wiring), and the real opener consumed by
  `parse_arith_expansion`.
- `ArithClose` — the `))` terminator.
- `ArithBail` — the not-arith signal: a `)` seen at `paren_depth == 0` that is
  NOT followed by `)`. Tells the parser to rewind and retry as command-sub.

Body content reuses existing atoms: `Lit { text, quoted }` for literal
expression runs, and the existing expansion openers `CmdSubOpen`, `ParamOpen`,
`BeginBacktick`, and a nested `ArithOpen`.

### New mode

`Mode::Arith { paren_depth: u32, in_dquote: bool, body_started: bool }`:
- `paren_depth` — grouping-paren nesting inside THIS arith frame (starts 0, i.e.
  just inside the consumed `$((`). Per-frame; captured by mark/rewind.
- `in_dquote` — the surrounding double-quote context, so body expansion atoms
  carry the correct `quoted` flag (mirrors the `ParamWordOperand { in_dquote }`
  pattern).
- `body_started` — false on a freshly pushed frame. The first `scan_step_arith`
  call (body_started=false) consumes the opening `$((` and emits the real
  `ArithOpen`, then flips body_started=true; subsequent calls scan the body.
  This mirrors v244's `Mode::CommandSub { body_started }`, where the mode's first
  scan consumes `$(` and emits `CmdSubOpen`. The parser positions the lexer at
  `$((` (cursor at `$`) before pushing the frame — both entry paths (the
  differential harness and the operand path) leave the cursor there.

### Nesting = separate frames

Each `$(( … ))` is independently delimited by its own `))` (no
escaping-compounding, unlike backtick). So a nested `$((` pushes its OWN
`Mode::Arith { paren_depth: 0, in_dquote }` frame; when it closes at its `))` the
frame pops and the outer frame's `paren_depth` resumes. The mode stack models
this directly — no shared/mutated single counter.

## Lexer: `scan_step_arith`

Dispatched from `scan_step` when the top mode is `Mode::Arith`. Emits body atoms
one at a time, forward-only, at most one char of peek:

- A run of ordinary expression text (digits, operators, whitespace, identifiers,
  `$` that is not an expansion opener, etc.) accumulates into a `Lit { text,
  quoted: in_dquote }` atom.
- A bare `(` → `paren_depth += 1`; the `(` is literal body text.
- A bare `)`:
  - `paren_depth > 0` → `paren_depth -= 1`; the `)` is literal body text (a
    grouping close).
  - `paren_depth == 0` → peek exactly one char (`peek_nth`): if `)` → emit
    `ArithClose` (consume both parens); otherwise → emit `ArithBail` (consume
    nothing beyond what identifies the signal; the parser rewinds).
- An expansion opener starts a recursion: `$(` → `CmdSubOpen`, `${` → `ParamOpen`,
  `` ` `` → `BeginBacktick`, `$((` → nested `ArithOpen`. The parser pushes the
  matching mode; those consume their own balanced delimiters, so their internal
  parens never reach `paren_depth`.
- EOF before a close → `LexError::UnterminatedArith` (matches the oracle).

`paren_depth` is lexer-owned because it decides token identity (is this `)`
literal body content, a grouping close, or the terminator/bail?). This is the
same rationale that made v245's backtick `depth` lexer-owned. The parser cannot
track it: at this layer the arith body is opaque literal text (the parser
assembles a `Word`, it does not parse arithmetic grammar — `arith.rs` does that
at runtime), so the grouping parens are characters, not structural tokens.

### The one-shot re-tokenize signal (for the wrinkle)

The lexer exposes a one-shot instruction — the parser sets it before re-pulling
on the bail path — that makes the NEXT `$((` tokenize as `$(` (emit `CmdSubOpen`,
consuming only `$(`, leaving the second `(` for the command-sub body to lex as a
subshell opener). This is a parser→lexer mode signal (the same information-flow
direction the mode stack already embodies), not a new "lexer depends on parser"
dependency. It is consumed exactly once and cleared.

## Parser: `parse_arith_expansion`

Signature parallel to `parse_backtick_sub` / `parse_command_sub`:
`parse_arith_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError>`.

1. **Mark** the lexer (before consuming the opener) so the wrinkle can rewind to
   the `$((` start.
2. Push `Mode::Arith { paren_depth: 0, in_dquote: quoted, body_started: false }`.
   The mode's first scan consumes the opening `$((` and emits the real
   `ArithOpen`, which the parser pulls. (Cursor is at `$((` on entry — the
   harness positions it there; the operand path leaves it there via a zero-width
   signal, see Operand wiring.)
3. Assemble the body `Word` with the word-part machinery: loop pulling atoms;
   `Lit` → push `WordPart::Literal`; each expansion opener → recurse
   (`parse_command_sub` / `parse_param_expansion` / `parse_backtick_sub` /
   `parse_arith_expansion`) and push the returned part; `ArithClose` → done.
4. On `ArithClose`: pop the mode, return `WordPart::Arith { body, quoted }`.
5. On `ArithBail` (or an EOF-in-the-bail-sense): pop the `Arith` mode, **rewind**
   to the mark, set the one-shot cmdsub re-tokenize signal, and re-drive the
   position as command substitution (`parse_command_sub`) so the `$(` opens a
   command-sub whose first token is the subshell `(`. Return that
   `WordPart::CommandSub`.
6. On any propagated `LexError` (e.g. `UnterminatedArith`): pop the mode on ALL
   paths before propagating (the v245-review lesson — wrap the fallible body so
   every exit flows through a single pop).

Mode push/pop parity: the `Arith` frame is popped on EVERY exit path (Ok / bail /
error), and nested recursion pushes/pops its own frames.

## Operand wiring

`scan_step_param_operand` currently emits `TokenKind::DeferredExpansion` for a
`$((` inside a `${…}` operand. v246 rewires that to emit a ZERO-WIDTH `ArithOpen`
signal — emitted WITHOUT consuming the `$((`, so the cursor stays put and
`parse_arith_expansion` (which pushes `Mode::Arith` whose first scan consumes
`$((`) runs unchanged. This matches the v244 `CmdSubOpen` / v245 `BeginBacktick`
operand pattern exactly. `parse_word` dispatches the `ArithOpen` signal to
`parse_arith_expansion(iter, in_dquote)`. Both operand sites (unquoted and
in-dquote) are updated. `operand_atoms` test helper adds `ArithOpen` to its
hand-off stop-list (the v245-review lesson: a zero-width signal emitted without
advancing the cursor spins a raw-lexer drive that has no parser to push the
mode).

## Error handling

- Unterminated `$(( … ` (EOF before close, with no bail) → `LexError::
  UnterminatedArith`, surfaced to the parser as `ParseError::Lex`, matching the
  oracle.
- The `$( (…) )` wrinkle is NOT an error — it is the mark/rewind fallback to a
  valid command substitution.
- Malformed inputs where the oracle and the new path might legitimately differ
  are pinned as documented deferred divergences (as in v245), not silently
  accepted.

## Testing

Differential harness mirroring v245's `old_bt`/`new_bt`/`diff_bt`:
- `old_arith(s, quoted)` — production oracle: tokenize `s` (optionally wrapped in
  `"…"`), pull the `WordPart::Arith` from the first Word token.
- `new_arith(s, quoted)` — `parse_arith_expansion` on a live lexer positioned at
  `$((`.
- `diff_arith(s)` — assert new == old for both unquoted and quoted.
- `diff_arith_ok(s)` — for the operand path (`${x:-$((…))}`) via the existing
  `diff_ok` helper.

Corpus (each verified byte-identical unless pinned):
- plain: `$((1+2))`, `$(( 1 + 2 ))`, `$((0))`, `$((-1))`;
- grouping parens: `$(( (1+2)*3 ))`, `$(( ((1+2)) ))`, `$(( a*(b+c) ))`;
- embedded expansions: `$(( $x + 1 ))`, `$(( ${y} ))`, `$(( $(echo 1) ))`,
  ``$(( `echo 1` ))``, and nested `$(( 3 * $((5*10)) ))`;
- quoted vs unquoted (`"$((1+2))"` vs `$((1+2))`);
- the wrinkle: `$((a)b)`, `$((cat) )`, `$( (a) )` written glued, `$((x)) `;
- unterminated: `$((1+2`, `$(( `;
- operand: `${x:-$((1+1))}`, `${x:+$((n))}`, `${x:-a$((i))b}`.

Any well-formed divergence discovered (as with v245's `\`-run) is pinned with a
dedicated `*_deferred` test + a `docs/bash-divergences.md` `[deferred]` entry and
reconciled before Stage-2 live-wiring; the differential corpus is the correctness
oracle.

## Task shape (for the plan)

Incremental SDD tasks, each dormant + differential + independently testable:
- **T1** — scaffolding: `ArithOpen`/`ArithClose`/`ArithBail` atoms,
  `Mode::Arith`, `parse_arith_expansion` skeleton, `old_arith`/`new_arith`/
  `diff_arith` harness, scaffolding test.
- **T2** — depth-0 body: literal runs, `ArithClose` terminator,
  `UnterminatedArith` on EOF; `$((1+2))` byte-identical.
- **T3** — grouping-paren depth + embedded expansions (`$var`/`${…}`/`$(…)`/
  backtick) in the body.
- **T4** — nested `$((` (separate frames).
- **T5** — the wrinkle: `ArithBail` + mark/rewind + the one-shot cmdsub
  re-tokenize retry; byte-identical `$( (…) )` fallback.
- **T6** — operand wiring (both sites + `operand_atoms` stop-list) + error
  parity + full single-threaded proof.

## Constraints

- `cargo test -p huck-syntax --jobs 1 -- --test-threads 1` only — the workspace
  suite OOM-kills this box (1 core / ~1.9 GiB) under parallel fan-out.
- Production scanners (`scan_arith_body`, `arith_string_to_word`,
  `scan_dollar_expansion`, and all other production paths) stay untouched.
- Every commit ends with the canonical trailer
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
