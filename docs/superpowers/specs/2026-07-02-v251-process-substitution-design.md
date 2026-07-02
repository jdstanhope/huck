# v251 — Process substitution (`<(…)`/`>(…)`) on the atom-command path (dormant, differential) — Design

**Status: APPROVED (2026-07-02).** Fourth Phase-C **Stage 2** "port a deferred
construct onto the atom-command path" iteration (after v248 funcdefs, v249
here-strings, v250 heredocs). Direction:
`2026-06-30-phase-c-parser-driven-frontend-roadmap.md` (Stage 2) + memory
`huck-frontend-parser-driven-direction` / `huck-lexer-rearch-design`.

## 1. Goal & context

huck's front-end is being inverted so the lexer emits small atoms and the
PARSER (`crates/huck-syntax/src/parser.rs`) assembles words + structure — a
DORMANT path (gated by a `command_atoms` lexer flag defaulting to `false`;
production still uses the batch Word-lexer + `command.rs` oracle) that must
produce ASTs byte-identical to the oracle, gated by the differential harness
`new_seq` (atoms) vs `old_seq` (oracle), with `diff_cmd(s)` asserting equality.
Each Stage-2 iteration removes one construct family from the atom path's
deferred set. v251 ports **process substitution (`<(cmd)` / `>(cmd)`)**.

Process substitution is one of the higher-impact, lower-risk remaining families:
the roadmap notes the `Mode::CommandSub` machinery already exists (v244), and a
procsub body is a paren-delimited command sequence IDENTICAL to a `$(…)` body —
so the port reuses that machinery wholesale. No new body-scanning logic.

## 2. What already exists (so the port is small)

- **Production** (the oracle, UNCHANGED): in the Word scanner, `<` (or `>`)
  followed by `(` is process substitution — it consumes `(`, calls
  `scan_paren_substitution` (the same body scanner as `$(…)`), and pushes
  `WordPart::ProcessSub { sequence, dir: ProcDir::In/Out }` onto the CURRENT word
  (a word part, NOT a redirect operator). Only produced when UNQUOTED (inside
  `"…"`/`'…'`, `<(`/`>(` are literal). `lexer.rs` ~2505 (`<(`) / ~2555 (`>(`).
- **AST** (UNCHANGED): `WordPart::ProcessSub { sequence: crate::command::Sequence,
  dir: ProcDir }` (`lexer.rs:327`); `ProcDir::In`/`Out`. Runtime expands it to a
  `/dev/fd/N` (or FIFO) path.
- **`Mode::CommandSub { body_started }`** (v244): the atom-path vehicle for a
  paren-delimited command-sub body. `scan_step_command_sub` (`lexer.rs:1671`):
  when `!body_started` the cursor sits on the opener — it consumes `$(`, emits
  the `CmdSubOpen` "real opener", and flips `body_started`; when `body_started`
  the body lexes via the shared command scanner with the PARSER owning the `)`.
- **`parse_command_sub`** (`parser.rs:765`): pushes `Mode::CommandSub`, consumes
  the `CmdSubOpen` real opener, does the empty-body check (`)` → the empty
  `Sequence` the oracle yields) else `parse_subshell_sequence` (with
  `zero_lines_in_sequence` + the `UnsupportedCommand → UnsupportedExpansion`
  mapping for body-deferred constructs), pops, returns
  `WordPart::CommandSub { sequence, quoted }`.
- **`parse_word_command`** (`parser.rs:118`): assembles a Word from atoms; already
  has a `CmdSubOpen` arm (consume signal → `parse_command_sub`) — the exact
  precedent the `ProcSubOpen` arm mirrors.
- **Current deferral:** the atom path defers procsub → `UnsupportedCommand`
  (`atoms_procsub_deferred`, `parser.rs:3154`).

So the port adds a `ProcSubOpen` signal + one opener branch + a `parse_process_sub`
mirroring `parse_command_sub`, and removes the deferral. No `command.rs`,
`scan_paren_substitution`, or production-scanner change.

## 3. Design

### 3.1 Lexer — disambiguation + body reuse (two additive edits)

New atom: `TokenKind::ProcSubOpen { dir: ProcDir }` — a zero-width WORD-PART
signal (dual to `CmdSubOpen`'s signal role; it carries the direction).

1. **`scan_command_operator_atom`, the `<` and `>` arms:** before the existing
   operator dispatch, add a `(`-lookahead branch: if the char after `<`/`>` is
   `(`, emit `ProcSubOpen { dir: In/Out }` (zero-width — cursor stays ON the
   `<`/`>`) and return WITHOUT running the operator `boundary_reset` tail (a
   procsub is a word continuation / word start, not an operator boundary). Every
   other case (`<<`, `<<<`, `<&`, `<>`, `<`, `>>`, `>&`, `>|`, `>`) is unchanged.
   THE RULE is obeyed: this is a one-char classify, not a forward scan for a
   matching delimiter — the `(…)` body is parser-driven via `Mode::CommandSub`.
2. **`scan_step_command_sub`, the `!body_started` opener branch:** widen it to
   accept a `<(`/`>(` opener in addition to `$(` — when the cursor is on `<`/`>`
   followed by `(`, consume BOTH chars, emit the `CmdSubOpen` real opener, and
   flip `body_started`. (The `dir` is NOT needed here — it is carried by the
   word-mode `ProcSubOpen` signal and tracked by the parser.) The body then lexes
   IDENTICALLY to a `$(…)` body.

Inside quotes the operator arm is never reached, so quoted `<(`/`>(` stays
literal automatically — matching the oracle's "only produced when UNQUOTED".

### 3.2 Parser — assemble the word part, remove the deferral

- **`parse_process_sub(iter, dir: ProcDir) -> Result<WordPart, ParseError>`** —
  mirrors `parse_command_sub`: push `Mode::CommandSub { body_started: false }`;
  `next_kind()?` must be the `CmdSubOpen` real opener (else pop +
  `UnsupportedExpansion`, matching the comsub deferral posture); empty-body check
  (`)` → the oracle's empty `Sequence`) else `parse_subshell_sequence` (with
  `zero_lines_in_sequence` + the `UnsupportedCommand → UnsupportedExpansion`
  mapping on body-deferred constructs); pop; return
  `WordPart::ProcessSub { sequence, dir }`. Differs from `parse_command_sub` ONLY
  in the return variant and in carrying `dir` instead of `quoted`.
- **`parse_word_command`** — new arm: `Some(TokenKind::ProcSubOpen { dir })` →
  capture `dir`, `next_kind()?` (consume the signal), `parts.push(
  parse_process_sub(iter, dir)?)`. Works identically for a fresh-word procsub
  (`cat <(x)`) and a glued one (`x<(y)`) — it is a word-part continuation.
- **Remove the deferral** that currently yields `UnsupportedCommand` for procsub,
  and retarget `atoms_procsub_deferred` to `diff_cmd` parity.
- **Redirect target** (`wc < <(cmd)`): comes for free — `parse_one_redirect`'s
  target assembly already routes through `parse_word_command`, which now handles
  `ProcSubOpen`.

## 4. Scope

**In scope** (all byte-identical to the oracle via `diff_cmd`, or matching-error
parity):

- Both directions: `<(cmd)` (In), `>(cmd)` (Out).
- Standalone: `cat <(echo hi)`, `tee >(cat)`.
- Glued to adjacent text/word-parts: `x<(y)`, `pre$(c)<(d)post`.
- As a redirect TARGET: `wc < <(sort f)`, `sort > >(uniq)`.
- Multiple on one command: `diff <(a) <(b)`, `echo <(a) >(b)`.
- Nested: `cat <( cat <(echo x) )`.
- Body containing pipelines / expansions / compounds (whatever
  `parse_subshell_sequence` handles for `$(…)`).
- Quoted → literal: `"<(x)"`, `'<(x)'`, `\<(x)` → NO procsub (matches oracle).
- Error / deferred parity: a body-deferred construct inside the procsub body
  (`<( [[ x ]] )`, `<( f() { :; } )`, a heredoc inside) → `UnsupportedExpansion`
  as for `$(…)`; malformed cases (`<(` at EOF, unterminated body) match the
  oracle (compare `new_seq` to `old_seq`, splitting lexer-level rejects that
  panic `old_seq` into an `is_err()`-only bucket — determine by observation, as
  the existing error-parity tests do).

**Out of scope / stays as-is.** The live flip (`command_atoms` stays `false`);
other deferred families (array literals, `[[ ]]`, arith command, coproc, `$[ ]`);
the v250 live-flip carry-forwards. NO `command.rs`, `scan_paren_substitution`, or
production-scanner change.

## 5. Invariants

- Byte-identical: every in-scope procsub input parses to the SAME AST / same error
  on the atom path as the oracle. A well-formed in-scope divergence is a v251 BUG
  to fix, not to pin.
- Production untouched: `command_atoms` defaults `false`; the production procsub
  scanner + `scan_paren_substitution` + `command.rs` + `process_line` unchanged;
  engine-facing `WordPart::ProcessSub`/`ProcDir` AST unchanged.
- The `<`/`>` arm change must NOT affect non-`(` redirects (`<`, `<<`, `<<<`,
  `<&`, `<>`, `>`, `>>`, `>&`, `>|`) — the existing redirect corpus stays green.
- 0 warnings; every commit carries the `Co-Authored-By: Claude Opus 4.8 (1M
  context)` trailer; branch `v251-process-substitution`, not `main`.

## 6. Implementation staging (~3 tasks)

1. Lexer: `TokenKind::ProcSubOpen { dir }` atom; the `<`/`>` operator-arm
   `(`-lookahead → emit the signal (no boundary_reset); widen
   `scan_step_command_sub`'s `!body_started` to accept `<(`/`>(`. DORMANT — parser
   still defers, so the gate is unchanged; verify at the atom-stream level that
   `<(`/`>(` emits `ProcSubOpen{dir}` (not a redirect operator) and that non-`(`
   `<`/`>` are unaffected.
2. Parser: `parse_process_sub` + the `parse_word_command` `ProcSubOpen` arm +
   remove the deferral; retarget `atoms_procsub_deferred` to `diff_cmd`. Green for
   the core positions (standalone both dirs, glued, redirect-target, multiple).
3. Full corpus: nested, body-with-expansions/pipelines/compounds, quoted-literal,
   adjacent-to-other-word-parts, and error/deferred parity (body-deferred
   construct, malformed). Full `huck-syntax` lib + doctests green, 0 warnings.
