# Incremental Lexer → Lexer/Parser Separation Re-architecture

**Status: IN PROGRESS (brainstorm ongoing).** This is the living design for a
multi-iteration front-end re-architecture. v237 (spanned tokens) shipped as
step 0. Sections marked OPEN are not yet decided. Update this doc as forks
resolve — it is the durable record so we do not re-discover the plan each
session.

## Why

`crates/huck-syntax/src/lexer.rs::tokenize_partial_inner` is one monolithic
batch loop carrying ~10 ad-hoc mode flags (`brace_expand`, `expect_regex`,
`dbracket_depth`, `in_assignment_value`, `enclosing_dquote`, `in_dquote`, …)
plus ~30 hand-written sub-scanners. Six brace scanners are drifted duplicates
(`scan_braced_param_expansion` / `_operand` / `_name` / `_name_ext` /
`_skip` / `scan_substitution_operand`) — the source of the v233–v235
parameter-expansion bug chain. The lexer is **batch** (whole input → `Vec<Token>`
up front), and command substitution re-enters via a recursive
`tokenize` + `parse` (`parse_substitution_body`). This is hard to reason about,
hard to extend, and the duplication keeps drifting.

## Target architecture (the destination)

1. **Incremental, pull-based lexer** — a `next_token()` engine instead of a
   batch loop. Tokens are produced one at a time on demand.
2. **True lexer/parser separation** — the parser drives the lexer rather than
   consuming a pre-baked `Vec<Token>`.
3. **Parser-controlled stack of lexers (modes)** — the parser pushes a context
   when it enters `$(`, `${`, `((`, `[[`, array `(`, etc., and pops on exit.
   Each context's "scan one token" logic is a separate component, replacing the
   ad-hoc flags and the drifted duplicate scanners.

This is reached in PHASES (see Migration), not all at once. "True separation"
is the eventual goal; incremental tokenization comes first.

## Section 1 — overall shape (settled)

- Pull-based token-at-a-time lexer with a **token history** buffer so the parser
  can **rewind** (lookahead/backtrack).
- Parser-driven **pure stack model**: the parser owns when contexts are
  pushed/popped.
- **Alias expansion** becomes a read-time concern via an injected
  `AliasResolver` (bash expands aliases while reading, at command position).
- **Foundation shipped:** v237 spanned tokens (`Token { kind, span }`), so
  location already travels with each token (needed for rewind + good errors).

## Section 2 — core: shared driver + mode producers (settled)

One thin `Lexer` driver owns the shared state; the stack holds lightweight mode
producers (each mode = a separate component that knows how to scan ONE token in
that context):

```rust
struct Lexer<'a> {
    cursor: CharCursor<'a>,   // single, shared char cursor (offset/line/column)
    history: Vec<Token>,      // produced tokens, for pull + rewind
    pos: usize,               // index into history of the next token to hand out
    modes: Vec<Mode>,         // the context stack; modes.last() drives next()
}
impl Lexer<'_> {
    fn next(&mut self) -> Token;          // produce/replay one token under top mode
    fn peek(&mut self, n: usize) -> &Token;
    fn push_mode(&mut self, m: Mode);
    fn pop_mode(&mut self);
    fn mark(&self) -> Mark;               // cheap checkpoint (history index)
    fn rewind(&mut self, m: Mark);        // O(1) index reset
}
// Mode = Command | ParamExpansion | Arith | CommandSub | Regex | ArrayLiteral | ...
```

Rationale: one global history → O(1) rewind by index; one shared cursor → token
offsets stay globally consistent (spans anchor to the root input, no sub-range
remapping); "modes as separate components" satisfies the "simplify the lexers
into separate components" goal without the overhead of independent sub-lexer
objects coordinating their own cursors/history.

Mode-change + rewind interaction (to nail down in Section 3): pushing/popping a
mode must invalidate any buffered lookahead produced under a different mode,
because the same bytes can lex differently per context. Rewind in practice is
WITHIN a single mode (assignment-vs-command, `name()` func-def detection), so a
"push/pop flushes lookahead past the mark" rule is expected to suffice.

## Migration — phased, low-risk (user-directed; settled in shape)

**Phase A — incremental engine behind the existing API.** Keep the public
`tokenize() -> Vec<Token>` (and `tokenize_with_opts`/`tokenize_partial`)
signatures UNCHANGED. Reimplement their bodies as a drain loop over the new
`next_token`/`Lexer` driver. The parser is UNTOUCHED — it still receives a
`Vec<Token>`. **Oracle:** the new incremental output must be byte-identical to
the current batch lexer; the full `cargo test --workspace` suite + the 152
`tests/scripts/*_diff_check.sh` harnesses already assert this transitively.
This proves the incremental engine in isolation, with zero parser risk.

Phase A is a **minimal extraction** (decided): lift the current
`tokenize_partial_inner` loop body into the driver with the smallest possible
diff — the ~10 local mode flags (`has_token`, `brace_expand`, `expect_regex`,
`dbracket_depth`, `in_assignment_value`, …) become `Lexer` fields; the loop body
becomes a `scan_step` that appends its 0..N produced tokens to `self.history`
(instead of a local `tokens` vec) and advances the cursor. The clean Mode-stack
decomposition (Section 3) is a SEPARATE follow-on (Phase A.2) layered on a proven
`next_token`, NOT done in Phase A. Mechanism — `next_token` drains the history
buffer, only scanning when it runs dry, so the existing "0 tokens (whitespace)"
and "N tokens (brace expansion)" iterations map cleanly:

```rust
fn next_token(&mut self) -> Result<Option<Token>, LexError> {
    loop {
        if self.pos < self.history.len() {           // hand out buffered token
            let t = self.history[self.pos].clone(); self.pos += 1; return Ok(Some(t));
        }
        match self.scan_step()? {                     // one old-loop iteration
            Step::Eof => return Ok(None),             // appends 0..N to history
            Step::Produced => continue,               // loop back to drain
        }
    }
}
// tokenize(): drain next_token() into a Vec<Token> — output byte-identical.
// Heredoc body backfill (collect_*_bodies mutating earlier Heredoc tokens) and
// tokenize_partial's partial+error contract both still work against `history`.
```

**Phase B — parser drives the lexer.** Once Phase A is verified, switch the
parser to pull from the `Lexer` (`next`/`peek`/`push_mode`/`pop_mode`/
`mark`/`rewind`) instead of consuming a `Vec<Token>`. This is where the
parser-controlled mode stack + rewind replace the ad-hoc flags, and command
substitution stops re-lexing recursively (the parser just pushes a CommandSub
mode). True lexer/parser separation lands here.

**Phase C (eventual, maybe) — lift expansion structure to the parser.** Today a
`"foo${x}bar"` is ONE Word token carrying `WordPart`s; the `${…}`/`$((…))`/`$(…)`
sub-structure is built by scanners inside the Word. A later phase could make the
parser see expansion structure as real tokens (full separation). Open whether
this is worth it — the Word/WordPart model is reasonable and Phase C is the
biggest change. Deferred; not required for A or B.

**Dependency-direction debt (surfaced by v239, fix in a later round).** Because
`parse_substitution_body` lives in `lexer.rs` and calls `command::parse`, the
lexer depends on the parser: `LexError::SubstitutionParseError` wraps a
`ParseError`. v239 adds the reverse edge — `ParseError::Lex(LexError)` so the
parser can surface a lex error hit mid-pull (e.g. a bad alias body) — which closes
the two enums into a **recursive type** (E0072). v239 breaks the cycle with a
`Box<LexError>` as a deliberate stopgap. The correct fix is to **remove the
lexer→parser edge**: the parser (not the lexer) parses `$(…)`/`${…}` bodies, so
`LexError` no longer contains a `ParseError` and the `Box` becomes unnecessary.
That is precisely Phase C (comsub stops recursive re-lex+parse inside the lexer).
Until then, keep the `Box`; do not add more lexer→parser calls.

## Section 3 — the mode set (OPEN)

Map the ~30 current scanners onto a small set of parser-pushable modes vs.
leaf helper scanners (e.g. `scan_squote_content`, `scan_hex_digits` stay
helpers). Candidate modes: Command (default), ParamExpansion (`${…}`),
Arith (`$((…))` / `((…))`), CommandSub (`$(…)` / backticks — recursive Command),
Regex (after `=~` in `[[…]]`), ArrayLiteral (`a=(…)`), DoubleBracket (`[[…]]`).
For Phase A, modes are INTERNAL (used to scan Words-with-WordParts so output
stays identical); they only become parser-visible in Phase B. — TO DESIGN.

## Section 4 — alias resolver (OPEN)

Read-time alias expansion via an injected `AliasResolver` trait at command
position. Today `alias_expand.rs` post-processes the token stream (v231/v232)
with a `_mapped` provenance map (now vestigial — spans carry location). In the
pull model the resolver hooks into Command mode when a command-position word
resolves to an alias: re-lex the alias body under the current mode stack, body
tokens inheriting the alias-name span (already implemented in v237's engine
path). — TO DESIGN (interface + where it lives).

## Open questions parking lot

- Exact `Mark`/rewind semantics across push/pop (flush rule).
- Heredoc body collection (side-effecting, line-oriented) in a pull model.
- Whether `tokenize_partial`'s error-recovery contract changes.
- Fate of `expand_aliases_in_tokens_mapped` (vestigial post-v237).
