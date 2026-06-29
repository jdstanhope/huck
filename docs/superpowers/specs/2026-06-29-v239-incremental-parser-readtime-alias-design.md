# v239 — Incremental parser: live pull + read-time alias folded into the lexer

**Iteration:** v239. **Umbrella design:**
`docs/superpowers/specs/2026-06-29-incremental-lexer-rearch-design.md`
(this is Phase B of that re-architecture — *the parser drives the lexer* — and it
folds alias resolution into the pull, which the umbrella doc had as the separate
S4 "AliasResolver". This design supersedes that: aliases live IN the lexer, the
parser supplies command position.)

## Goal

Make the parser **genuinely incremental**: it pulls `Token`s **live** from the
`Lexer` one at a time, scanning on demand, with **no up-front `Vec<Token>`**.
Alias expansion folds into the lexer's pull: the parser tells the lexer when it is
at a command position, and the lexer expands a registered alias *in place*,
emitting the body's tokens one at a time before resuming. The whole-token-vector
alias **pre-pass** (`huck-engine`) and `TokenCursor` are retired.

Output stays **byte-identical** for every input. The win is structural: real
incremental tokenization driven by the parser, alias resolution at the single
place that already knows command position (the parser), and the elimination of
the duplicate command-position logic the pre-pass reimplemented.

## Why this shape

- **No cross-crate inversion / no trait needed.** Aliases reach the lexer as plain
  data (`String → String`); command position comes from the parser; both the
  parser (`command.rs`) and lexer (`lexer.rs`) live in `huck-syntax`. The parser
  just holds `&mut Lexer`. No `TokenSource` trait, no decorator, no injected
  resolver.
- **No duplicate command-position logic.** Today's `Expander`
  (`huck-engine/alias_expand.rs`) reimplements reserved-word / `case` / `for` /
  `[[ ]]` / `name()` tracking *only* to locate command position. The parser
  already knows command position from its grammar, so it supplies it. We move only
  the *expansion mechanics* into the lexer and delete the rest.
- **Cross-unit def-then-use falls out for free.** Because the pull is lazy, a
  later unit is not scanned until earlier units have executed, so an alias defined
  by one unit is already present when the lexer reaches the next unit. The v231
  `alias_generation` re-tokenize hack for def-then-use retires.

## Core invariant — live, incremental, byte-identical

1. **The parser pulls `Token`s live from the lexer; it holds no `Vec`.** At any
   mid-parse point, input past the lexer's cursor is unscanned. (v238's
   `next_token` already scans lazily — this is the consumer of it.)
2. **Alias expansion happens inside the lexer's pull at command position only.**
   The parser signals command position; the lexer checks its alias map there.
3. **Byte-identical output** — same `Sequence`, same errors, same exit codes — for
   every input. The existing suite (esp. the `alias*` harnesses) is the oracle.

## What changes

### 1. Lexer: command-position pull + alias map

The `Lexer` (made `pub`) gains a parser-facing pull API, alongside the existing
internal `next_token`. The pull is **fallible: every scanning method returns
`Result<…, LexError>`** — a lex error hit while scanning is **returned at the
failing call**, not stashed. `impl From<LexError> for ParseError` (it wraps
`Lex(Box<LexError>)`) makes each of the ~150 parser sites `iter.peek_kind()?`: the
`?` propagates and converts the error. (`Box` because `LexError` already contains a
`ParseError` via `SubstitutionParseError`, so an unboxed `ParseError::Lex(LexError)`
would be a recursive type — E0072. Removing that lexer→parser edge is deferred to
Phase C of the re-arch; until then the `Box` stays.) This replaces the v239-T1
`pending_error`/`take_error` stash, which was a side-channel hack.

The **primary** pull yields whole `Token`s (kind **+ span**), so location travels
into the parser; **kind convenience** methods serve the many sites that only match
on kind, so the parser-side change at those sites is `iter.peek()` →
`iter.peek_kind()?` — a near-mechanical rename plus a `?`:

```rust
// primary: whole Token (span included)
pub fn peek(&mut self)  -> Result<Option<&Token>, LexError>;
pub fn next(&mut self)  -> Result<Option<Token>, LexError>;
// kind convenience — the find-replace targets for the existing peek/next sites
pub fn peek_kind(&mut self)  -> Result<Option<&TokenKind>, LexError>;
pub fn peek2_kind(&mut self) -> Result<Option<&TokenKind>, LexError>;
pub fn next_kind(&mut self)  -> Result<Option<TokenKind>, LexError>;
// location
pub fn peek_span(&mut self) -> Result<Option<Span>, LexError>;
pub fn current_line(&mut self) -> Result<u32, LexError>;
pub fn remaining(&self) -> usize;   // non-scanning; replaces TokenCursor::len
// command position: expand a registered alias IN PLACE, then behave like
// peek/next. The alias body is lexed, so these are fallible too.
pub fn peek_command_kind(&mut self) -> Result<Option<&TokenKind>, LexError>;
pub fn next_command(&mut self) -> Result<Option<Token>, LexError>;
// refreshable alias map (§3) — NO take_error; errors are returned, not stashed
pub fn set_aliases(&mut self, aliases: HashMap<String, String>);
```

`Result<Option<T>, LexError>` encodes three distinct outcomes the parser must
tell apart: `Err` = scan failed; `Ok(None)` = clean end of input (NOT an error);
`Ok(Some(t))` = next token. The two layers are orthogonal — `Result` = "did it
error", `Option` = "token or end" — so neither collapses into the other.

Construction takes the alias map and options:
`Lexer::new(input, aliases, opts)` (or `new_with_aliases`; `brace_expand` stays a
constructor argument as today).

### 2. Alias expansion mechanics (moved into the lexer)

`peek_command`/`next_command`, when the upcoming word is an unquoted bareword that
matches the alias map:

- **Lex the alias body** (the lexer already tokenizes; the body is lexed with the
  current options) and push its tokens to the front of the pull (into `history`
  ahead of `pos`), so they drain one at a time.
- **Expand before the reserved-word check.** Because the parser calls
  `peek_command` *before* it decides reserved-word-ness, `alias x='if'` correctly
  yields the `if` reserved word.
- **Recursion guard.** An "actively expanding" set prevents an alias from
  expanding itself into a loop (bash rule).
- **Trailing-blank eligibility.** If an alias value ends in a blank, the *next*
  word is also alias-eligible at command position (bash rule) — tracked across the
  expansion.
- **Quoted / non-command words are never expanded.** Only an unquoted command-
  position word is a candidate.

These mechanics are ported from the current `Expander`; its command-position
*detection* (reserved words / context stack / `name()`) is **deleted** — the
parser supplies command position.

### 3. Refreshable aliases across units (def-then-use)

The `Lexer` *owns* its alias map. An alias defined by one unit must be visible to
a *later* unit (`alias foo=bar⏎foo` in a sourced file), while a same-unit
definition does not take effect (`alias foo=bar; foo` leaves `foo` unexpanded —
bash semantics).

- The `source`/`-c` loop does *parse one unit → execute → next unit*. After each
  unit executes (the `alias`/`unalias` builtins run there), it calls
  `lexer.set_aliases(shell.aliases.clone())` before parsing the next unit — gated
  on the shell's existing alias-generation counter so the clone happens only when
  aliases actually changed.
- Because the lexer *owns* the map (not a live `&shell.aliases` borrow held across
  the loop), the executor freely takes `&mut shell` between units — no borrow
  conflict.
- The REPL builds a fresh `Lexer` per line with the current aliases; no mid-parse
  refresh needed.
- Same-unit non-expansion is preserved naturally: within one `parse_one_unit`, the
  alias map is not refreshed, and the def is executed only after the unit parses.

### 4. Parser: pull live, signal command position, surface lex errors at the entry

- `command::parse` and `parse_one_unit` take `&mut Lexer`. Alias-expanding callers
  (REPL, `source` loop) build a live `Lexer::new_live(input, &aliases, opts)`;
  alias-free callers (continuation, comsub, function-body, prompt) keep `tokenize`
  and wrap the vec in `Lexer::from_tokens` (replay) — byte-identical.
- Internal `fn …(iter: &mut TokenCursor)` parser functions take `iter: &mut Lexer`
  (same parameter name). The ~150 `iter.peek()`/`iter.peek2()`/`iter.next()` sites
  become `iter.peek_kind()?`/`iter.peek2_kind()?`/`iter.next_kind()?` — a rename plus
  a `?` (the cursor is always the single `iter` param, so replacing those exact
  strings is safe). `peek_span`/`current_line` also gain `?`; `iter.len()` (source
  loop) becomes `iter.remaining()` (no `?` — non-scanning). A few helpers that pull
  and currently return `bool`/`()` must become `Result<_, ParseError>`; the compiler
  enumerates this bounded cascade.
- **At command position** — the grammar point(s) where the parser is about to read
  a command name (`command.rs:1115` in `parse_command_inner`, `2381` in
  `parse_next_stage`) — it calls `peek_command_kind`/`next_command` instead of
  `peek_kind`/`next_kind`, so the lexer expands a registered alias there.
  Everywhere else (arguments, operators, redirections) the plain `*_kind` variants
  are used, so no alias expansion happens off command position.
- **Error model — fallible pull, `?`-propagated.** Every scanning pull method
  returns `Result<…, LexError>`; a `LexError` hit while pulling (main line *or*
  alias body) is **returned at the failing call** and propagates through `?`,
  converting to `ParseError::Lex(Box<LexError>)` via `From`. No stash, no
  `take_error`. The outcome is the same as today (a lex error wins, `process_line` →
  `Continue(2)`), now expressed as ordinary error propagation. (See Risks for the
  one incremental-vs-batch behavioral note: on a line carrying *both* an early parse
  error and a later lex error, the incremental parser reports the parse error first
  — like bash — whereas today's batch lexer reports the lex error; verify against the
  harnesses.)

### 5. Retire `TokenCursor`, the alias pre-pass, and the re-tokenize hack

- Delete `TokenCursor` (`command.rs`) — its buffer/position/lookahead role is the
  `Lexer`'s `history`/`pos`.
- Delete `huck-engine`'s alias pre-pass entry points
  (`expand_aliases_in_tokens` / `_mapped`) and the v231 `alias_generation`
  re-tokenize path; their command-position logic is gone and their expansion
  mechanics now live in the lexer.

## Affected call sites

| Path | Today | After |
|------|-------|-------|
| REPL (`shell.rs:process_line_in_sinks`) | tokenize → pre-pass → `parse(vec)` | build `Lexer::new(line, aliases, opts)`; `parse(&mut lx)` live |
| `source`/`-c` (`builtins.rs`) | tokenize_partial → pre-pass → `parse_one_unit` loop | one `Lexer` over the chunk; `parse_one_unit(&mut lx)` loop; `set_aliases` between units; between-unit `span.offset` read from the lexer |
| continuation (`continuation.rs`) | tokenize → clone → `parse` ×2 | two `Lexer`s over the same string (re-lex) for the two parses |
| command-sub (`lexer.rs:parse_substitution_body`) | tokenize body → `parse(vec)` | tokenize body → `Lexer::from_tokens` replay → `parse(&mut lx)`; NO alias map (matches current huck — `$()` bodies are not alias-expanded) |
| function body (`shell_state.rs`) | tokenize → `parse(vec)` | `Lexer::new(body, empty_or_aliases, opts)`; `parse(&mut lx)` |
| prompt cmdsub (`prompt.rs`) | tokenize → `parse(vec)` | `Lexer::new(...)`; `parse(&mut lx)` |

## Mechanics that must be preserved

1. **2-token lookahead only.** The parser uses at most `peek`/`peek2`; `history`
   buffering makes that trivial. (No backtracking/rewind exists in the parser.)
2. **Speculative assignment-peel.** The parser's bounded internal re-synthesis of
   already-consumed assignment Words (`prefix_tokens`, for `FOO=1 BAR=2 [[`) is
   retained as-is — it re-processes consumed tokens, it is not a token source.
3. **`source` offset slicing.** Between-unit `span.offset` (for `set -v` echo and
   error reporting) is read from the live lexer between `parse_one_unit` calls;
   offsets are unchanged (spans are set at lex time).
4. **Same-line / same-unit alias non-expansion** and **cross-unit def-then-use**
   per §3.
5. **Aliases inside `$(...)` are NOT expanded** — unchanged from current huck (the pre-pass never reached inside `$()` bodies either). The comsub sub-parse uses a replay lexer with no alias map.
6. **`$LINENO` / span correctness** through alias bodies — body tokens inherit the
   alias-name span, as the pre-pass did.

## Verification

Oracle (byte-identical):

- `cargo test --workspace` — any divergence in token consumption or alias
  resolution breaks parser/expansion/executor tests.
- All 152 `tests/scripts/*_diff_check.sh` harnesses (release), with the **alias**
  harnesses (`alias*.sub` and friends) as the critical lens — they exercise
  command-position eligibility, recursion, trailing-blank, def-then-use, and
  expansion inside compounds/`case`/functions.

Direct tests:

1. **Command-position expansion.** `peek_command`/`next_command` expand a
   registered alias; `peek`/`next` (argument position) do not.
2. **Expand-before-reserved-word.** `alias x='if'` then `x cond; then …` parses as
   an `if`.
3. **Recursion guard.** A self-referential alias terminates.
4. **Trailing-blank eligibility.** `alias a='b '; alias c=…; a c` expands `c` too.
5. **Quoted not expanded.** `'ll'` / `"ll"` at command position is literal.
6. **Cross-unit def-then-use.** `alias foo=bar⏎foo` (two units) expands; `alias
   foo=bar; foo` (one unit) does not.
7. **Incremental pull (parser-level).** Parsing consumes tokens without a
   materialized vec — assert the lexer cursor stays near the consumed prefix
   mid-parse (à la v238's laziness proof, but driven by the parser).
8. **Propagated lex error.** A bad alias body (`alias x='echo "'`; `x`) surfaces as
   `ParseError::Lex` via `?` from the failing pull, with the same effect as today
   (`syntax error`, exit 2).
9. **Parity** for `source` multi-unit + offset slicing, continuation double-parse, and that `$(...)` bodies are still NOT alias-expanded (no behavior change).

## Risks & mitigations

- **Largest change surface in the series** (parser signatures + live pull + alias
  relocation across crates + several consumer reworks). Mitigation: **internal
  phasing** (below) so intermediate commits stay byte-identical; lean on the
  oracle, especially the alias harnesses.
- **Alias-rule fidelity.** Moving expansion mechanics risks regressing subtle bash
  rules (recursion, trailing-blank, reserved-word interplay). Mitigation: port the
  `Expander`'s mechanics faithfully; the `alias*` harnesses + the direct tests
  above pin them; keep the v232 regression cases as explicit tests.
- **Cursor-rename breadth + cascade.** ~150 `iter.peek()/peek2()/next()` sites are
  renamed to the `*_kind` variants **and given `?`**. The rename is exact-string
  find-replace (the cursor is always the single `iter` param); the `?` triggers a
  bounded cascade where a few pulling helpers that returned `bool`/`()` must become
  `Result<_, ParseError>`. The compiler enumerates both a wrongly-`?`'d non-cursor
  `iter` and the helper cascade. Do it in one pass under the oracle; never
  `.unwrap()` a pull to silence the cascade.
- **Incremental error ordering.** Going incremental means a line with *both* an
  early parse error and a later lex error now reports the parse error first (like
  bash), versus today's batch lexer reporting the lex error. Mitigation: verify
  against the harnesses; if a harness pins the old order, treat it as an
  intentional, bash-aligned change and note it.
- **`peek` borrow ergonomics.** `peek`/`peek_kind` take `&mut self` (fill-then-
  borrow) and return a borrow of `history`; NLL handles it.

## Internal phasing (implementation strategy; detailed in the plan)

To keep intermediate states byte-identical:

0. **Result-ize the pull API** (supersedes the v239-T1 `Option`+stash): convert the
   pull methods + `fill_to` to `Result<…, LexError>`, delete `pending_error`/
   `take_error`. lexer-only; nothing calls them yet but the unit tests — byte-identical.
1. **Flip the parser to pull from `&mut Lexer`** (the `*_kind?` pull + `peek_span?`/
   `current_line?`/`remaining`) with `ParseError::Lex(Box<LexError>)` + `From`, the
   alias pre-pass still running and feeding a `from_tokens` replay lexer. Rename + `?`
   + bounded helper cascade; behavior-preserving under the oracle.
2. **Make the pull live and fold alias in:** add `peek_command_kind`/`next_command`
   + the alias map + expansion mechanics; have the parser call the command-position
   variants at the two sites; switch the REPL to live `Lexer::new_live(...)`; retire
   the pre-pass, `TokenCursor`, and the `alias_generation` hack; add `set_aliases` +
   the between-unit refresh. Byte-identical, now incremental.
3. **Edge consumers:** rework continuation (replay lexers), confirm `source`
   multi-unit offset slicing, the `$(...)`-not-expanded invariant, and that a bad
   alias body propagates as `ParseError::Lex` via `?`.

## Definition of done

- The parser pulls from `&mut Lexer` **live**, holds no `Vec`, and calls
  `peek_command_kind`/`next_command` exactly at command position.
- Alias expansion happens inside the lexer's command-position pull; the
  `huck-engine` pre-pass, the `alias_generation` re-tokenize hack, and
  `TokenCursor` are deleted.
- The lexer owns a refreshable alias map; the multi-unit loop re-syncs it between
  units; cross-unit def-then-use works and same-unit defs do not expand.
- The pull is fallible (`Result<…, LexError>`); lex errors (main line and alias
  bodies) propagate via `?` to `ParseError::Lex(Box<LexError>)`, with today's effect.
  No `pending_error`/`take_error` stash.
- `Lexer` is `pub`; `parse`/`parse_one_unit` take `&mut Lexer`; `new_live` builds
  the alias-expanding REPL/source lexer; all callers updated.
- All direct tests pass; `cargo test --workspace` green; all 152 harnesses green
  (release), alias harnesses included; 0 warnings.
