# Spanned Tokens (v237) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold source location into each token (`Token { kind, span }`,
`Span { offset, line, column }`) and retire the three parallel lockstep vectors
(tokens / byte-offsets / lines) the lexer emits today.

**Architecture:** `enum Token` is renamed `enum TokenKind`; a new `Token` struct
pairs a `TokenKind` with a `Span`. `CharCursor` gains 1-based character-column
tracking. The lexer emits self-locating tokens, so `offsets`/`lines`/sentinel/
`push_pos` disappear; `tokenize_with_offsets` and `parse_with_lines` collapse
into `tokenize_with_opts`/`parse`. Alias-body tokens inherit the alias-name
token's span (fixing `$LINENO` through alias expansion). The `Sequence` AST
shape is unchanged, so the executor and huck-cli are untouched.

**Tech Stack:** Rust workspace (`huck-syntax`, `huck-engine`, `huck-cli`).

## Global Constraints

- **Behavior-preserving refactor.** The full `cargo test --workspace` (~3878
  tests) is the regression firewall and must end green. The single intentional
  behavior change is `$LINENO` becoming correct for alias-expanded commands
  (was `0`).
- **Span shape (verbatim):** `Span { offset: usize, line: u32, column: u32 }`.
  `column` is a **1-based character column** (Unicode scalars from line start;
  a tab counts as 1; reset to 1 after each consumed `'\n'`).
- **`Token { kind: TokenKind, span: Span }`.** `Token`'s `PartialEq`/`Eq`/`Hash`
  compare **only `kind`** — span is positional metadata, not token identity.
  This is a deliberate, documented design choice (it also keeps equality-based
  lexer tests valid).
- **`TokenKind` keeps `#[non_exhaustive]`** and the existing
  `#[derive(Debug, PartialEq, Eq, Clone)]`.
- Removed public API: `lexer::tokenize_with_offsets`, `command::parse_with_lines`.
  Callers move to `lexer::tokenize_with_opts` / `command::parse`.
- Run the full suite with `cargo test --workspace` (plain `cargo test` skips
  crates). Per-crate gate: `cargo test -p huck-syntax`.

---

## File Structure

- `crates/huck-syntax/src/lexer.rs` — `Span`, `TokenKind`/`Token`, `CharCursor`
  column, span-emitting lexer, simplified `tokenize*` API, helper/test updates.
- `crates/huck-syntax/src/command.rs` — `TokenCursor` over spanned tokens,
  `parse` stamps `Command.line` from spans, `parse_with_lines` removed.
- `crates/huck-syntax/src/lib.rs` — re-exports (`Token`, `TokenKind`, `Span`
  added; removed fns dropped).
- `crates/huck-syntax/examples/tokenize_dump.rs` — updated to spanned tokens.
- `crates/huck-engine/src/alias_expand.rs` — span inheritance; map retained.
- `crates/huck-engine/src/shell.rs` — simplified `process_line_in_sinks`.
- `crates/huck-engine/src/builtins.rs` — `tokenize_partial` 2-tuple; mapped
  alias call relies on inherited spans.
- `crates/huck-engine/src/{expand,continuation}.rs` — any genuine lexer-`Token`
  field/match sites updated to `.kind`.
- Tests: new column unit tests (lexer.rs), retargeted line-stamp test
  (command.rs), new alias-`$LINENO` integration test (huck-engine).

**TDD note (refactor):** genuinely-new behavior (column tracking, alias
`$LINENO`) is written test-first. The pervasive type migration is
compiler-driven with the existing suite as the firewall — "write a failing
test" for a rename is replaced by "the suite must stay green," stated per task.

---

## Task 1: `Span` type + `CharCursor` character-column tracking

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (CharCursor struct + `new` + `next` + accessors; add `Span`)
- Test: `crates/huck-syntax/src/lexer.rs` (unit tests near CharCursor)

**Interfaces:**
- Produces: `pub struct Span { pub offset: usize, pub line: u32, pub column: u32 }`
  with `Span::unknown()` (`{0,0,0}`); `CharCursor::column(&self) -> u32`.

- [ ] **Step 1: Write the failing column test**

Add to the lexer test module:

```rust
#[test]
fn char_cursor_tracks_offset_line_column() {
    let mut c = CharCursor::new("ab\ncé\td");
    // before consuming: at 'a'
    assert_eq!((c.offset(), c.line(), c.column()), (0, 1, 1));
    c.next();                       // consume 'a'
    assert_eq!((c.offset(), c.line(), c.column()), (1, 1, 2)); // at 'b'
    c.next();                       // consume 'b'
    assert_eq!((c.offset(), c.line(), c.column()), (2, 1, 3)); // at '\n'
    c.next();                       // consume '\n' -> next line, col resets
    assert_eq!((c.offset(), c.line(), c.column()), (3, 2, 1)); // at 'c'
    c.next();                       // consume 'c'
    assert_eq!((c.offset(), c.line(), c.column()), (4, 2, 2)); // at 'é' (2 bytes)
    c.next();                       // consume 'é' -> offset +2, column +1
    assert_eq!((c.offset(), c.line(), c.column()), (6, 2, 3)); // at '\t'
    c.next();                       // consume tab -> one column
    assert_eq!((c.offset(), c.line(), c.column()), (7, 2, 4)); // at 'd'
}
```

- [ ] **Step 2: Run it to confirm it fails to compile** (`column` undefined)

Run: `cargo test -p huck-syntax char_cursor_tracks_offset_line_column`
Expected: compile error — no method `column`.

- [ ] **Step 3: Add `Span` and column tracking**

Add the `Span` type near the `Token` definitions:

```rust
/// A token's source location. `column` is a 1-based character column
/// (Unicode scalars from the line start; a tab is one column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub offset: usize,
    pub line: u32,
    pub column: u32,
}

impl Span {
    /// Placeholder span for synthesized tokens / test fixtures (line 0 = unknown).
    pub fn unknown() -> Span { Span { offset: 0, line: 0, column: 0 } }
}
```

Add `column` to `CharCursor` (field + init + accessor) and update `next()` so
column advances per character and resets after a newline:

```rust
pub struct CharCursor<'a> {
    s: &'a str,
    pos: usize,
    line: u32,
    column: u32,            // NEW: 1-based character column
    peeked: Option<char>,
    peeked_len: usize,
}

// in new(): column: 1,
// add accessor:
/// 1-based character column of the next char to be produced.
pub fn column(&self) -> u32 { self.column }
```

In `next()`, both the peeked-fast-path and the slow path must update column the
same way — after consuming `c`:

```rust
if c == '\n' { self.line += 1; self.column = 1; } else { self.column += 1; }
```

(Replace the existing `if c == '\n' { self.line += 1; }` in both arms.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p huck-syntax char_cursor_tracks_offset_line_column`
Expected: PASS.

- [ ] **Step 5: Confirm no regressions**

Run: `cargo test -p huck-syntax`
Expected: PASS (Span/column are additive; no token change yet).

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v237 T1: Span type + CharCursor character-column tracking"
```

---

## Task 2: `Token { kind, span }` across huck-syntax (lexer + parser)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (types, emit sites, `tokenize*` API, test helpers)
- Modify: `crates/huck-syntax/src/command.rs` (`TokenCursor`, `parse`, remove `parse_with_lines`)
- Modify: `crates/huck-syntax/src/lib.rs` (re-exports)
- Modify: `crates/huck-syntax/examples/tokenize_dump.rs`

**Interfaces:**
- Consumes: `Span` (Task 1).
- Produces: `pub struct Token { pub kind: TokenKind, pub span: Span }`;
  `pub enum TokenKind` (old `Token` body); `Token::new(kind, span)`,
  `impl From<TokenKind> for Token` (span `unknown()`), `Token::kind(&self)`.
  `tokenize_with_opts(&str, LexerOptions) -> Result<Vec<Token>, LexError>`
  (unchanged signature; `Token` now spanned). `tokenize_partial(&str,
  LexerOptions) -> (Vec<Token>, Option<(LexError, usize)>)`.
  `TokenCursor::new(Vec<Token>) -> Self`; `TokenCursor::peek(&self) ->
  Option<&TokenKind>`, `peek2`, `peek_span(&self) -> Option<Span>`,
  `current_line(&self) -> u32`; `next(&mut self) -> Option<Token>`.
  `command::parse(Vec<Token>) -> Result<Option<Sequence>, ParseError>`.

> This is the large mechanical task. Execute the steps in order; the compiler
> drives the remaining edits, and the existing suite is the firewall. **Do not
> change lexing/parsing behavior** — only how location is carried.

- [ ] **Step 1: Define `TokenKind` and `Token`**

Rename the enum and add the struct (keep attrs on the enum):

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum TokenKind {
    // ... existing variants verbatim ...
}

/// A token paired with its source location. Equality and hashing are by
/// `kind` only — `span` is positional metadata, not part of token identity.
#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Token { Token { kind, span } }
    pub fn kind(&self) -> &TokenKind { &self.kind }
}

impl From<TokenKind> for Token {
    fn from(kind: TokenKind) -> Token { Token { kind, span: Span::unknown() } }
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool { self.kind == other.kind }
}
impl Eq for Token {}
impl std::hash::Hash for Token {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) { self.kind.hash(h); }
}
```

(If `TokenKind` is not already `Hash`, add `Hash` to its derive only if some
consumer requires it; otherwise omit the `Hash for Token` impl. Verified: no
`HashSet<Token>`/`HashMap<Token,_>` exists today, so `Hash` is optional —
include it only if the build asks for it.)

- [ ] **Step 2: Rename enum-variant references to `TokenKind::` in both files**

In `lexer.rs` and `command.rs`, replace the **variant path** `Token::` with
`TokenKind::` (construction and match arms alike). Use a scoped substitution and
review the diff (do not touch the new `Token` struct, `Token::new`,
`Token::kind`, `Token::from`, or `TokenCursor`):

Run (review before committing):
```bash
sed -i 's/\bToken::/TokenKind::/g' crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/command.rs
# then hand-fix the few intended `Token`-the-struct references the sed over-rewrote:
#   Token::new / Token::kind / Token::from  ->  restore to Token::
```

- [ ] **Step 3: Emit spanned tokens in the lexer**

Replace the `offsets`/`lines`/sentinel/`push_pos` machinery with span capture at
each token's start. At every emit site, the token's start offset/line/column are
already known (the existing `token_start` for words; the current char position
for operators/newlines). Construct the span there and push a `Token`:

```rust
// pattern at an emit site (operator/newline example):
let span = Span { offset: c_off, line: c_line, column: c_col };
out.push(Token::new(TokenKind::Op(op), span));
```

For word emission, capture `(token_start, token_start_line, token_start_column)`
when the word's first char is seen (mirror the existing `token_start` logic; add
a `token_start_column`). Brace-expanded words that emit N tokens reuse the same
start span for each (matching today's `push_pos` repetition).

Remove: the `offsets`/`lines` local vectors, all `push_pos` / `push_pos_into`
calls, the sentinel push, and the `offsets.len() == tokens.len() + 1`
debug_asserts.

Update the public producers:
- `tokenize` / `tokenize_with_opts` — bodies now return `Vec<Token>` directly
  (signatures unchanged).
- `tokenize_partial` — return `(Vec<Token>, Option<(LexError, usize)>)`; delete
  `PartialTokens`'s two position vectors and `TokensWithPos`.
- **Delete `tokenize_with_offsets`** and the `TokensWithPos` type alias.

- [ ] **Step 4: Migrate `TokenCursor` and `parse`; remove `parse_with_lines`**

In `command.rs`:

```rust
pub struct TokenCursor {
    tokens: Vec<Option<Token>>,   // spanned
    pos: usize,
}
impl TokenCursor {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens: tokens.into_iter().map(Some).collect(), pos: 0 }
    }
    /// Span of the next token (None past end).
    pub fn peek_span(&self) -> Option<Span> {
        self.tokens.get(self.pos)?.as_ref().map(|t| t.span)
    }
    /// Line of the next token (0 if unknown / past end) — for $LINENO stamping.
    pub fn current_line(&self) -> u32 {
        self.peek_span().map(|s| s.line).unwrap_or(0)
    }
    /// Peek the next token's KIND without consuming.
    pub fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos)?.as_ref().map(|t| &t.kind)
    }
    pub fn peek2(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos + 1)?.as_ref().map(|t| &t.kind)
    }
    // next() returns the owned spanned Token; callers that only need the kind
    // destructure `.kind`.
    pub fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get_mut(self.pos)?.take();
        if t.is_some() { self.pos += 1; }
        t
    }
    // ... preserve any other existing methods, adapted to the field change ...
}
```

The parser's `peek()/peek2()` match sites now compare against `TokenKind::…`
(handled by Step 2's rename). Sites that consumed an owned `Token` (e.g.
extracting a `Word`) destructure the kind: `match cur.next() { Some(Token { kind:
TokenKind::Word(w), .. }) => …, … }`. The compiler enumerates these; they are
few relative to the peek-matches.

Update `parse`:
```rust
pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    let mut cur = TokenCursor::new(tokens);
    parse_cursor(&mut cur)
}
```
Delete `parse_with_lines`. `Command.line` is stamped from `cur.current_line()`
exactly where `parse_with_lines` did it (the stamping call sites are unchanged;
only their line source moves from the parallel vector to the token span).

- [ ] **Step 5: Update `lib.rs` re-exports**

Export `Token`, `TokenKind`, `Span`; drop `parse_with_lines` and
`tokenize_with_offsets` from the re-export lists.

- [ ] **Step 6: Update lexer test helpers + inline literals + the example**

Update the five token-building helpers so they wrap into `Token` (span
`unknown()` — equality ignores it):

```rust
fn w(s: &str) -> Token { Token::from(TokenKind::Word(Word(vec![WordPart::Literal(s.into())]))) }
// wq, wqd, vword_unquoted, words: same pattern — build the TokenKind, then `.into()`.
```

Remaining inline `vec![TokenKind::Newline, …]` / `vec![TokenKind::Word(…)]` test
literals: wrap each element with `Token::from(...)` (or `.into()`), or compare
against `TokenKind` via a small `kinds(&toks)` helper where that reads cleaner.
Because `Token: PartialEq` ignores span, `assert_eq!(tokenize(x).unwrap(),
vec![w("echo"), …])` keeps working once the helpers return `Token`.

Update `examples/tokenize_dump.rs` to the new token shape (print `tok.kind` and
`tok.span`).

- [ ] **Step 7: Build and run the huck-syntax suite to green**

Run: `cargo build -p huck-syntax && cargo test -p huck-syntax`
Expected: PASS. Fix compiler-reported sites until green. (The workspace build is
intentionally still broken — huck-engine is migrated in Task 3.)

- [ ] **Step 8: Commit**

```bash
git add crates/huck-syntax
git commit -m "v237 T2: Token { kind, span } across huck-syntax; drop parallel arrays + redundant API"
```

---

## Task 3: Migrate huck-engine consumers; alias span-inheritance ($LINENO fix)

**Files:**
- Modify: `crates/huck-engine/src/alias_expand.rs`
- Modify: `crates/huck-engine/src/shell.rs:370-414`
- Modify: `crates/huck-engine/src/builtins.rs` (~6350, ~6387)
- Modify: `crates/huck-engine/src/{expand,continuation}.rs` (only genuine lexer-`Token` sites)
- Test: `crates/huck-engine` (new alias-`$LINENO` integration test)

**Interfaces:**
- Consumes: Task 2's `Token`/`TokenKind`, `tokenize_with_opts`,
  `tokenize_partial` (2-tuple), `parse`.
- Produces: `expand_aliases_in_tokens(Vec<Token>, &HashMap<String,String>) ->
  Result<Vec<Token>, LexError>` (alias-body tokens carry inherited spans);
  `expand_aliases_in_tokens_mapped` keeps returning `(Vec<Token>, Vec<usize>)`.

- [ ] **Step 1: Write the failing alias-`$LINENO` integration test**

Create `crates/huck-engine/tests/alias_lineno.rs` (or add to an existing engine
integration test file) asserting that a command produced by alias expansion
reports its real source line, and a non-alias multi-line case is unchanged. Use
the crate's existing harness for running a script and capturing output (mirror a
nearby integration test's setup). Assert the alias-expanded `$LINENO` equals the
line the alias use appears on (not `0`).

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -p huck-engine alias_lineno`
Expected: FAIL (today alias expansion zeroes the line → `$LINENO` is `0`), or
compile failure pending Step 3's API migration.

- [ ] **Step 3: Migrate `alias_expand.rs` with span inheritance**

The `Expander` already tracks, per output token, the source token it came from
(the `map`). Stamp the inherited span there: when an alias name expands, each
re-tokenized body token's `span` is set to the **alias-name token's span**;
untouched tokens keep their own span. Keep the `(out, map)` return of
`expand_aliases_in_tokens_mapped` intact (consumed by `builtins.rs:6387` and 3
unit tests). `Token::` variant references in this file become `.kind`
matches / `TokenKind::` construction (compiler-driven). Re-tokenizing an alias
value uses `tokenize_with_opts`; overwrite each resulting token's span with the
name token's span before pushing.

- [ ] **Step 4: Simplify `shell.rs::process_line_in_sinks`**

Replace the tokenize→slice→alias→parse block with:

```rust
let opts = lexer::LexerOptions { extglob: shell.extglob(), ..Default::default() };
let tokens = match lexer::tokenize_with_opts(line, opts) {
    Ok(t) => t,
    Err(e) => { /* same syntax-error path as today */ return ExecOutcome::Continue(2); }
};
let tokens = if expand_aliases {
    match crate::alias_expand::expand_aliases_in_tokens(tokens, &shell.aliases) {
        Ok(t) => t,
        Err(e) => { /* same syntax-error path */ return ExecOutcome::Continue(2); }
    }
} else { tokens };
match command::parse(tokens) {
    Ok(Some(sequence)) => executor::execute_with_sink(&sequence, shell, line, sink, err_sink),
    Ok(None) => ExecOutcome::Continue(0),
    Err(e) => { /* same parse-error path */ ExecOutcome::Continue(2) }
}
```

Gone: `_offsets`, the `lex_lines[..tokens.len()]` sentinel slice, and the
`if t.len() == lines.len() { … } else { vec![0; …] }` line-zeroing. Keep the
exact error-message text and return codes.

- [ ] **Step 5: Update `builtins.rs`**

`~6350`: `tokenize_partial` now returns `(Vec<Token>, Option<(LexError, usize)>)`
— update the destructure and any use of the dropped offset/line vectors (derive
positions from `token.span` if needed). `~6387`: the
`expand_aliases_in_tokens_mapped` call still returns `(tokens, map)`; line info
now rides on the tokens' inherited spans, so drop any manual line remap that used
the map for `$LINENO` and let the spans carry it. `Token::` → `.kind`/`TokenKind::`
as the compiler directs.

- [ ] **Step 6: Sweep remaining engine `Token` sites**

Build the workspace; for each genuine lexer-`Token` site the compiler flags in
`expand.rs` / `continuation.rs` (ignore `ArithToken`/other types), switch
matches/field access to `.kind` / `TokenKind::`.

Run: `cargo build --workspace`
Expected: clean build.

- [ ] **Step 7: Run the new test and the full suite**

Run: `cargo test -p huck-engine alias_lineno` → PASS.
Run: `cargo test --workspace` → PASS (~3878 tests green).

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine
git commit -m "v237 T3: migrate huck-engine to spanned tokens; alias span-inheritance fixes \$LINENO"
```

---

## Self-Review

- **Spec coverage:** Span+column (T1); `Token{kind,span}` + retired parallel
  arrays + removed `tokenize_with_offsets`/`parse_with_lines` (T2); engine
  ripple + alias span-inheritance `$LINENO` fix + map retained (T3). All spec
  sections map to a task.
- **Placeholder scan:** none — new code is shown in full; the pervasive rename is
  specified as an exact transformation rule + compiler-driven cleanup, which is
  the correct way to plan a workspace-wide type migration (transcribing 680
  call-site edits would be noise).
- **Type consistency:** `Token`/`TokenKind`/`Span` names, `tokenize_with_opts`/
  `tokenize_partial`(2-tuple)/`parse` signatures, and `TokenCursor`'s
  `peek -> &TokenKind` / `next -> Token` / `current_line` are used consistently
  across T2 and T3. `expand_aliases_in_tokens_mapped` keeps `(Vec<Token>,
  Vec<usize>)` (resolves the spec's audit: the map is consumed, so it stays).
```
