# v237: spanned tokens — fold location into the token, retire the parallel arrays

**Status:** approved (brainstorm 2026-06-29)
**Iteration:** v237

## Goal

Make every token carry its own source location, replacing the three parallel
vectors (`tokens` / byte-`offsets` / `lines`) that the lexer currently emits in
lockstep. A token becomes `Token { kind, span }` with
`Span { offset, line, column }`. This retires the lockstep machinery and the
sentinel asymmetry that is a standing source of bugs, and it is the prerequisite
foundation for the larger front-end re-architecture (token-at-a-time lexer +
parser-driven stack of context lexers) that follows in a later iteration.

This iteration is a **behavior-preserving refactor**. The one intentional,
beneficial side effect (alias-expanded commands gaining correct `$LINENO`
instead of `0`) is called out explicitly below.

## Background: the parallel structure today

`crates/huck-syntax/src/lexer.rs` produces tokens and their positions as
**separate, parallel vectors** kept in lockstep by a `push_pos` call at ~40
emit sites:

- `type TokensWithPos = (Vec<Token>, Vec<usize> /*byte offset*/, Vec<u32> /*1-based line*/)`
- `type PartialTokens = (Vec<Token>, Vec<usize>, Vec<u32>, Option<(LexError, usize)>)`

The public producers are `tokenize` (`Vec<Token>` only), `tokenize_with_opts`
(`Vec<Token>` only), `tokenize_with_offsets` (`TokensWithPos`, with the error
arm carrying a byte offset), and `tokenize_partial` (`PartialTokens`).

**Why it is error-prone (verified):**

- The offset/line vectors carry a **trailing sentinel** —
  `offsets.len() == tokens.len() + 1` — but the parser's `TokenCursor::new`
  requires `lines.len() == tokens.len()` (no sentinel). Every consumer must
  remember to `lines.truncate(tokens.len())` / `lex_lines[..tokens.len()]`
  before parsing. That exact mismatch caused three separate bugs in the prior
  (parked) iteration's session.
- A defensive `debug_assert_eq!(offsets.len(), lines.len(), "offsets/lines out
  of lockstep")` exists only because lockstep drift is a real failure mode.
- **Column is not tracked at all** — only byte offset + line. There is no place
  to put a column without a parallel *fourth* vector.

**Cross-crate consumers of the parallel structure** (the blast radius):

- `crates/huck-engine/src/shell.rs:379,406` — `process_line_in_sinks` drives
  `tokenize_with_offsets` → alias-expand → `parse_with_lines`. It discards the
  error offset (`_off`), discards `_offsets`, slices the line sentinel
  (`lex_lines[..tokens.len()]`), and **zeroes all line info when alias
  expansion changes the token count** (`vec![0; t.len()]`).
- `crates/huck-engine/src/alias_expand.rs` — `expand_aliases_in_tokens` and
  `expand_aliases_in_tokens_mapped`. The `_mapped` variant returns a per-output
  `Vec<usize>` mapping each output token to the source token it came from,
  precisely so callers can remap byte-offsets/lines back to the raw source
  after expansion rewrites the stream (used by the non-interactive `source`
  loop).
- `crates/huck-engine/src/builtins.rs:6350` — one `tokenize_partial` caller.
- `crates/huck-syntax/src/command.rs` — `TokenCursor` holds a parallel `lines:
  Vec<u32>`; `parse_with_lines(tokens, lines)` stamps `Command.line` (read by
  the executor for `$LINENO`); `parse(tokens)` builds with `vec![0; n]`
  ("unknown" lines).

## Scope

**In scope:**

- New `Span { offset: usize, line: u32, column: u32 }` and
  `Token { kind: TokenKind, span: Span }`; today's `enum Token` is renamed
  `enum TokenKind` (contents unchanged).
- `CharCursor` tracks a **1-based character column** (Unicode scalar count from
  line start; tab counts as 1; reset to 1 after each consumed `'\n'`), alongside
  the existing offset and line.
- Lexer emits `Vec<Token>` with each token self-locating. The `push_pos`
  lockstep machinery, the parallel `offsets`/`lines` vectors, and the trailing
  sentinel are **removed**.
- `command.rs`: `TokenCursor` consumes spanned tokens and reads line (and now
  column/offset, available though not yet consumed) from each token; `Command`
  line-stamping for `$LINENO` is preserved, sourced from the token span.
- huck-engine consumers updated: `shell.rs` driver, `alias_expand.rs`,
  `builtins.rs:6350`.
- Public huck-syntax API simplified (see Components): redundant
  `tokenize_with_offsets` and `parse_with_lines` are removed; callers move to
  `tokenize_with_opts` / `parse`.

**Out of scope (explicitly NOT this iteration — belongs to the re-arch):**

- The token-at-a-time lexer interface, the rewind/history buffer, the
  parser-driven stack of context lexers, and moving alias expansion *into* the
  command-context lexer via an injected resolver. v237 keeps the existing
  eager-`Vec` lex → token-stream alias pass → parse pipeline; it only changes
  how each token carries its location.
- Consuming the new `column` for diagnostics. Column is captured and plumbed,
  but no error message reads it yet. (It exists so the re-arch and later
  diagnostics work can use it.)
- Any change to lexing/parsing *behavior* beyond the alias-`$LINENO` side
  effect below.

## Definition of done

- Full **`cargo test --workspace`** (~3878 tests) green. This is a pure
  refactor; the existing suite is the regression firewall.
- No parallel offset/line vectors remain in the lex→parse→alias path; no
  sentinel, no `push_pos` lockstep, no `truncate`/`[..len]` slice dance.
- `Token` carries `{offset, line, column}`; column is correct (unit-tested
  across multibyte and multi-line input).
- `$LINENO` unchanged for non-alias commands; for alias-expanded commands it
  becomes the correct source line (was `0`) — asserted by a new test.

## Architecture & data flow

```text
&str
  → CharCursor  (offset, line, column; column resets after '\n')
  → lexer       → Vec<Token>            each Token { kind, span }
  → alias pass  → Vec<Token>            alias-body tokens inherit the alias-NAME
                                        token's span; untouched tokens keep theirs
  → parser      → Sequence              Command.line stamped from token span
  → executor    (unchanged AST shape)
```

The `Sequence`/`Command`/`Word` AST output shape is unchanged (`Command.line`
remains a `u32`, now sourced from the token's span), so the execution core,
`expand.rs`, most of `builtins.rs`, and all of huck-cli are untouched.

## Components

1. **`Span` + `Token`** (`lexer.rs`):
   - `pub struct Span { pub offset: usize, pub line: u32, pub column: u32 }`
     with a small constructor and `Copy`/`Clone`/`Debug`/`PartialEq`.
   - `pub enum TokenKind { … }` — the current `enum Token` body verbatim.
   - `pub struct Token { pub kind: TokenKind, pub span: Span }` with helpers as
     needed (e.g. `Token::new(kind, span)`, and a way to compare *kind* in the
     many existing tests that pattern-match tokens — see Testing/migration).

2. **`CharCursor` column tracking** (`lexer.rs`):
   - Add `column: u32` (start 1). In `next()`, after consuming a char: if it was
     `'\n'`, set `column = 1` (and `line += 1` as today); else `column += 1`.
   - Add `pub fn column(&self) -> u32`. `offset()`/`line()` unchanged. The
     `peeked`/`peeked_len` fast path must update column identically.

3. **Lexer emission** (`lexer.rs`): replace the `offsets`/`lines` vectors +
   `push_pos` + sentinel with direct `Token { kind, span }` construction at each
   emit site, where `span = Span { offset, line, column }` captured at the
   token's first char (the existing `token_start` / start-of-token offset, now
   joined by the start line and start column). `tokenize`, `tokenize_with_opts`
   return `Result<Vec<Token>, LexError>` unchanged in signature.
   `tokenize_partial` returns `(Vec<Token>, Option<(LexError, usize)>)` (drops
   the two position vectors and the sentinel; the error byte-offset is retained
   for its callers).

4. **Public API simplification** (`lexer.rs` / `command.rs` / `lib.rs`):
   - **Remove `tokenize_with_offsets`** — redundant now that `tokenize_with_opts`
     yields self-locating tokens. Its sole external caller (`shell.rs`) used
     neither the separate offsets nor the error offset.
   - **Remove `parse_with_lines`** — redundant now that tokens carry line.
     `parse(tokens)` stamps `Command.line` from token spans. `TokenCursor::new`
     takes only `Vec<Token>`.
   - Update `lib.rs` re-exports accordingly (`Token`, `TokenKind`, `Span` newly
     exported; `parse_with_lines` / `tokenize_with_offsets` removed).

5. **`alias_expand.rs`**: operate on `Vec<Token>` (spanned). Alias-body tokens
   (re-tokenized from the alias value) **inherit the span of the alias-name
   token they replaced**; untouched tokens keep their own spans. This is exactly
   what the existing `_mapped` source-index map computes — so the position remap
   becomes span inheritance done in-place inside the expander. The separate
   `Vec<usize>` map return is retired *unless* a remaining caller needs it for a
   non-span purpose (resolve during planning by auditing `_mapped` callers); the
   default is to remove it.

6. **`shell.rs` `process_line_in_sinks`**: collapses to
   `tokenize_with_opts` → (optional) `expand_aliases_in_tokens` → `parse`. The
   sentinel slice, the `_offsets` discard, and the count-mismatch `vec![0; …]`
   line-zeroing all disappear — alias-body tokens now carry correct (inherited)
   spans, so `$LINENO` is correct through alias expansion.

7. **`builtins.rs:6350`**: update the lone `tokenize_partial` caller to the new
   2-tuple shape.

## Error handling

- `LexError` is unchanged. The lex-error byte offset previously surfaced by
  `tokenize_with_offsets`'s `Err((e, off))` was unused by `shell.rs`; callers
  that *did* want an error offset use `tokenize_partial`'s retained
  `Option<(LexError, usize)>`.
- Parser error reporting is unchanged.

## Edge cases & behavior parity

| Case | Behavior |
|---|---|
| Multibyte chars (`héllo`) | column counts Unicode scalars, so `column` advances by 1 per char even though `offset` advances by the byte width. Unit-tested. |
| Tabs | a tab is one column (no tab-stop expansion). Documented choice. |
| `'\n'` handling | after a consumed newline, the next token's span has `line+1`, `column=1`. The `Newline` token's own span is at the newline's own offset/line/column (as `offset`/`line` are today). |
| Empty input | `Vec::new()`; no sentinel to special-case. |
| `$LINENO`, non-alias | identical to today (line from token span == old `lines[i]`). |
| `$LINENO`, alias-expanded | **changes 0 → correct line** (beneficial side effect of span inheritance). New test asserts it. |
| `tokenize_partial` callers | get tokens that self-locate; the dropped position vectors are reconstructable from `token.span` if ever needed. |

## Testing

1. **Full `cargo test --workspace` is the firewall.** Every existing lexer and
   parser test must pass. Where tests pattern-match on `Token::Word(..)` etc.,
   provide a migration path (a `kind()` accessor or a `t.kind` field match) so
   the assertions target `TokenKind`; prefer a small mechanical update over
   loosening assertions.
2. **New unit tests** in `lexer.rs`:
   - `Span`/column correctness: single line, multi-line, multibyte, tab, and a
     token after a newline (asserts `{offset, line, column}` triples for a known
     input).
   - `tokenize_partial` returns correct spans on the tokens-before-error.
3. **New unit test** in `command.rs`: `parse` stamps `Command.line` from token
   spans (the case `parse_with_lines_stamps_exec_command_lines` previously
   covered, retargeted to `parse`).
4. **New integration test** (engine): an alias whose expansion changes the
   token count, asserting the expanded command's `$LINENO` equals the real
   source line (the beneficial change), and a non-alias multi-line `$LINENO`
   case unchanged.
5. No new bash-diff harness is required (no user-visible behavior surface
   changes except the `$LINENO` improvement, covered by 4).

## Risks & migration

- **Primary risk: a wrong column/line at an emit site silently corrupts spans.**
  Mitigated by capturing the span from the same `token_start` the lexer already
  tracks (offset is provably unchanged from today; line was already stamped
  per-token), adding only column, and unit-testing the triples.
- **Mechanical breadth: ~40 emit sites + every `Token::` pattern-match in
  tests.** This is the bulk of the work and is transcription, not design. Tasks
  are sized so each is independently testable; the suite gates each.
- **`_mapped` caller audit** (planning step): confirm no caller needs the raw
  source-index map for a non-span purpose before removing it. If one does,
  retain the map return and layer span inheritance on top.
- **Follow-on:** none deferred to `docs/bash-divergences.md` — this is internal
  refactor, not a divergence fix. The re-arch iteration that builds on this is
  tracked separately (brainstorm Section 1 settled; Sections 2–5 pending).
```
