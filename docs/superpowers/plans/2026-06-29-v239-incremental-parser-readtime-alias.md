# v239 — Incremental parser + read-time alias in the lexer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the parser pull tokens **live** from the `Lexer` one at a time (no up-front `Vec`), and fold alias expansion into the lexer's command-position pull, retiring the `huck-engine` alias pre-pass, the `alias_generation` re-tokenize hack, and `TokenCursor`.

**Architecture:** The `Lexer` (already incremental via v238's `next_token`) gains a parser-facing pull API (`peek`/`next` yielding whole `Token`s, plus `*_kind` convenience methods, `peek_command_kind`/`next_command`, `peek_span`/`current_line`/`remaining`, `take_error`, `set_aliases`). The parser holds `&mut Lexer` and drives it; at the two command-position sites it calls the `*_command` variants and the lexer expands a registered alias in place. The pull is **infallible at the call sites** — a `LexError` is stashed and surfaced once at the entry points as `ParseError::Lex`.

**Tech Stack:** Rust. Crates `huck-syntax` (`lexer.rs`, `command.rs`) and `huck-engine` (`shell.rs`, `builtins.rs`, `continuation.rs`, `shell_state.rs`, `prompt.rs`, `alias_expand.rs`).

**Spec:** `docs/superpowers/specs/2026-06-29-v239-incremental-parser-readtime-alias-design.md`

## Global Constraints

- **Byte-identical output** for every input — same `Sequence` AST, same errors, same exit codes. Oracle: `cargo test --workspace` (currently 3872 tests) + all 152 `tests/scripts/*_diff_check.sh` harnesses (release binary, `HUCK_BIN=target/release/huck`), the `alias*` harnesses the critical lens.
- **Run the FULL suite:** `cargo test --workspace` (plain `cargo test` skips most crates). 0 warnings.
- **Pull stays infallible at call sites** (returns `Option`, not `Result`); lex errors are stashed and checked via `take_error()` at the parse entry points → `ParseError::Lex`. No `?` threading through the ~150 parser pull sites.
- **Primary pull yields whole `Token`s** (kind + span); `*_kind` convenience methods serve the kind-only sites so the parser change there is a pure mechanical rename.
- **Alias rules preserved verbatim** from `Expander`: recursion guard (`active` set), trailing-blank eligibility (`body.chars().last().is_whitespace()`), quoted/non-literal words never match (`simple_word_text` returns `None`), alias body tokens inherit the alias-name span/src for `$LINENO`, expansion happens before the parser's reserved-word check.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Work on branch `v239-incremental-parser` (create from `main`).

## File map

- `crates/huck-syntax/src/lexer.rs` — `Lexer` struct (now `pub`), pull API, `from_tokens`, stash, alias storage + command-position expansion, `parse_substitution_body` rewrite.
- `crates/huck-syntax/src/command.rs` — `ParseError::Lex`, parser drives `&mut Lexer`, delete `TokenCursor`, command-position calls, entry-point `take_error`.
- `crates/huck-engine/src/shell.rs` — REPL builds a live `Lexer`.
- `crates/huck-engine/src/builtins.rs` — `source`/`-c` loop drives a live `Lexer`, between-unit `set_aliases`, retire `alias_generation` gate.
- `crates/huck-engine/src/continuation.rs` — two lexers for the double-parse.
- `crates/huck-engine/src/shell_state.rs`, `prompt.rs` — build a live `Lexer`.
- `crates/huck-engine/src/alias_expand.rs` — pre-pass entry points deleted; expansion mechanics referenced when porting (Task 3).

---

### Task 1: Lexer parser-facing pull API + replay constructor + error stash

Add the infallible pull API the parser will drive, plus a `from_tokens` replay constructor (used by Task 2's mechanical flip while the alias pre-pass still runs). No parser changes yet; behavior unchanged (new code is exercised only by unit tests).

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (struct ~490, `new` ~511, after `next_token` ~1201)
- Test: `crates/huck-syntax/src/lexer.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: existing `Lexer { history: Vec<Token>, pos, … }`, `fn next_token(&mut self) -> Result<Option<Token>, LexError>`, `fn scan_step`, `Step`, `Token`, `TokenKind`, `Span`, `LexError`.
- Produces (all `pub`, on `Lexer`):
  - `pub fn from_tokens(tokens: Vec<Token>) -> Lexer<'static>`
  - `fn fill_to(&mut self, idx: usize)` (private) — pull via `next_token` until `history.len() > idx` or EOF/err; on `Err`, store into `self.pending_error` and stop.
  - `pub fn peek(&mut self) -> Option<&Token>`, `pub fn next(&mut self) -> Option<Token>`
  - `pub fn peek_kind(&mut self) -> Option<&TokenKind>`, `pub fn peek2_kind(&mut self) -> Option<&TokenKind>`, `pub fn next_kind(&mut self) -> Option<TokenKind>`
  - `pub fn peek_span(&mut self) -> Option<Span>`, `pub fn current_line(&mut self) -> u32`, `pub fn remaining(&self) -> usize`
  - `pub fn take_error(&mut self) -> Option<LexError>`
  - New field `pending_error: Option<LexError>`; new field `replay: bool`.
  - `Lexer` and `LexerOptions` made `pub`; `Lexer::new` stays `fn` (internal) but add `pub fn from_tokens`.

- [ ] **Step 1: Make `Lexer` pub and add the two new fields**

In `lexer.rs`, change `struct Lexer<'a> {` → `pub struct Lexer<'a> {` and add fields (after `pos: usize,`):
```rust
    /// Lex error captured mid-pull (read-time path). Surfaced once via take_error().
    pending_error: Option<LexError>,
    /// True for a from_tokens() replay lexer: history is pre-filled, never scans.
    replay: bool,
```
In `Lexer::new`, initialize `pending_error: None,` and `replay: false,`.

- [ ] **Step 2: Write failing tests for the pull API**

Add to the `tests` module:
```rust
#[test]
fn pull_api_reproduces_token_sequence() {
    let toks = tokenize("echo foo | grep bar").unwrap();
    let mut lx = Lexer::from_tokens(toks.clone());
    assert_eq!(lx.remaining(), toks.len());
    assert_eq!(lx.peek_kind(), Some(&toks[0].kind));
    assert_eq!(lx.peek2_kind(), Some(&toks[1].kind));
    assert_eq!(lx.peek_span(), Some(toks[0].span));
    let mut drained = Vec::new();
    while let Some(t) = lx.next() { drained.push(t); }
    assert_eq!(drained, toks);
    assert_eq!(lx.peek_kind(), None);
    assert_eq!(lx.next_kind(), None);
    assert!(lx.take_error().is_none());
}

#[test]
fn pull_next_kind_matches_next_dot_kind() {
    let toks = tokenize("a b c").unwrap();
    let mut lx = Lexer::from_tokens(toks.clone());
    assert_eq!(lx.next_kind(), Some(toks[0].kind.clone()));
    assert_eq!(lx.peek_kind(), Some(&toks[1].kind));
}
```

- [ ] **Step 3: Run tests — verify they fail to compile** (`from_tokens`/`peek_kind` undefined)

Run: `cargo test -p huck-syntax pull_api 2>&1 | head`
Expected: compile error, methods not found.

- [ ] **Step 4: Implement `from_tokens`, `fill_to`, and the pull methods**

After `next_token` (≈line 1201) inside `impl<'a> Lexer<'a>`, add:
```rust
/// Build a replay lexer over already-tokenized input (Task 2 bridge). history is
/// pre-filled; scanning is a no-op so the pull never errors.
pub fn from_tokens(tokens: Vec<Token>) -> Lexer<'static> {
    Lexer {
        cursor: CharCursor::new(""),
        opts: LexerOptions::default(),
        brace_expand: true,
        history: tokens,
        pos: 0,
        pending_error: None,
        replay: true,
        parts: Vec::new(),
        current: String::new(),
        has_token: false,
        token_start: 0,
        token_start_line: 1,
        token_start_col: 1,
        in_assignment_value: false,
        dbracket_depth: 0,
        expect_regex: false,
        pending_heredocs: std::collections::VecDeque::new(),
    }
}

/// Ensure history[idx] exists AND is backfill-ready (heredoc body present),
/// pulling lazily via scan_step. Mirrors next_token's readiness check so a
/// Heredoc token is never exposed before its body is collected (v238 rule).
/// On a lex error, stash it and stop — the pull then reports end-of-input.
/// scan_step appends to history WITHOUT advancing pos, so this never consumes.
fn fill_to(&mut self, idx: usize) {
    if self.replay || self.pending_error.is_some() {
        return;
    }
    loop {
        if self.history.len() > idx && !self.backfill_pending_at(idx) {
            return;
        }
        match self.scan_step() {
            Ok(Step::Produced) => {}
            Ok(Step::Eof) => return,
            Err(e) => { self.pending_error = Some(e); return; }
        }
    }
}

pub fn peek(&mut self) -> Option<&Token> {
    self.fill_to(self.pos);
    self.history.get(self.pos)
}
pub fn next(&mut self) -> Option<Token> {
    self.fill_to(self.pos);
    let t = self.history.get(self.pos).cloned();
    if t.is_some() { self.pos += 1; }
    t
}
pub fn peek_kind(&mut self) -> Option<&TokenKind> {
    self.fill_to(self.pos);
    self.history.get(self.pos).map(|t| &t.kind)
}
pub fn peek2_kind(&mut self) -> Option<&TokenKind> {
    self.fill_to(self.pos + 1);
    self.history.get(self.pos + 1).map(|t| &t.kind)
}
pub fn next_kind(&mut self) -> Option<TokenKind> {
    self.next().map(|t| t.kind)
}
pub fn peek_span(&mut self) -> Option<Span> {
    self.fill_to(self.pos);
    self.history.get(self.pos).map(|t| t.span)
}
pub fn current_line(&mut self) -> u32 {
    self.peek_span().map(|s| s.line).unwrap_or(0)
}
pub fn remaining(&self) -> usize {
    self.history.len().saturating_sub(self.pos)
}
pub fn take_error(&mut self) -> Option<LexError> {
    self.pending_error.take()
}
```

Do **not** rename or change `next_token` — it stays as the v238 drain primitive that `tokenize_partial_inner` (and the v238 tests) call. The parser pull uses `fill_to` (which calls `scan_step` to *extend* `history` without advancing `pos`) plus `next()`/`peek()` which read/advance `pos` directly. `next_token` and the new `next()` both advance `pos` but are never mixed on the same lexer (the parser drives the pull API; `tokenize*` drives `next_token`).

Note: in a `replay` lexer (`from_tokens`), `fill_to` early-returns so `scan_step` is never reached; `history` is pre-filled and `next`/`peek` just index it. The public `tokenize*` path builds a non-replay lexer as today and is unaffected.

- [ ] **Step 5: Run tests — verify pass**

Run: `cargo test -p huck-syntax pull_api pull_next_kind 2>&1 | tail -5`
Expected: 2 passed.

- [ ] **Step 6: Full crate + workspace sanity, commit**

Run: `cargo test -p huck-syntax 2>&1 | grep "test result"` then `cargo build --workspace 2>&1 | grep -c warning` (expect 0).
```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v239 T1: Lexer parser-facing pull API + from_tokens replay + error stash

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Flip the parser (and source loop) to drive `&mut Lexer`; delete `TokenCursor`

Mechanically rename the parser's cursor from `TokenCursor` to `Lexer` and the pull calls to the `*_kind` variants. `parse(tokens)` keeps its `Vec<Token>` signature (builds `from_tokens` internally) so most callers are untouched; the `source` loop swaps `TokenCursor::new` → `Lexer::from_tokens`. Add `ParseError::Lex` and the entry-point `take_error` check. The alias pre-pass still runs → byte-identical.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (`TokenCursor` 773-817, `parse_cursor` 819, `parse` 833, `parse_one_unit` 897, `ParseError` 731-758, all 44 `&mut TokenCursor` fns)
- Modify: `crates/huck-engine/src/builtins.rs` (source loop ≈6412 `TokenCursor::new`, ≈6452/6460/6571 `peek_span`, `iter.len()` ≈6461)
- Modify: `crates/huck-syntax/src/errors.rs` (parse-error message arm)

**Interfaces:**
- Consumes (from Task 1): `Lexer::from_tokens`, `peek_kind`, `peek2_kind`, `next_kind`, `peek_span`, `current_line`, `remaining`, `take_error`. `Lexer` is `pub`.
- Produces: `pub enum ParseError { …, Lex(LexError) }`; `pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError>` (unchanged signature, builds `from_tokens`); `pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError>`. `TokenCursor` removed.

- [ ] **Step 1: Add `ParseError::Lex` + message arm**

In `command.rs`, add to `enum ParseError` (after `ArithForHeader(String),`):
```rust
    /// NEW (v239): a lex error surfaced while the parser pulled tokens live
    /// (e.g. a bad alias body). Carries the inner lexer error.
    Lex(crate::lexer::LexError),
```
In `crates/huck-syntax/src/errors.rs`, find `parse_error_message_impl` and add an arm mirroring the existing lex-error formatting, e.g.:
```rust
ParseError::Lex(e) => crate::lex_error_message(e),
```
(Check the exact helper name in `errors.rs`; reuse whatever `lex_error_message`/`lex_error_message_impl` the file exposes so the text matches today's lex-error output.)

- [ ] **Step 2: Delete `TokenCursor`; rewrite `parse`/`parse_cursor`/`parse_one_unit`**

Replace `struct TokenCursor { … }` + its `impl` + `impl Iterator` + `impl ExactSizeIterator` (command.rs:773-817) with nothing (delete). Rewrite the three entry functions:
```rust
fn parse_cursor(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    skip_newlines(iter);
    if iter.peek_kind().is_none() {
        return Ok(None);
    }
    let seq = parse_sequence(iter, &[])?;
    if let Some(e) = iter.take_error() {
        return Err(ParseError::Lex(e));
    }
    if iter.peek_kind().is_some() {
        return Err(ParseError::UnexpectedToken);
    }
    Ok(Some(seq))
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    let mut iter = Lexer::from_tokens(tokens);
    parse_cursor(&mut iter)
}

pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    while matches!(iter.peek_kind(), Some(TokenKind::Newline)) {
        iter.next_kind();
    }
    if iter.peek_kind().is_none() {
        return Ok(None);
    }
    let seq = parse_sequence_opts(iter, &[], true)?;
    if let Some(e) = iter.take_error() {
        return Err(ParseError::Lex(e));
    }
    Ok(Some(seq))
}
```
Add `use crate::lexer::Lexer;` at the top of `command.rs` if not present.

- [ ] **Step 3: Mechanical rename across `command.rs`**

(a) Parameter type: every `iter: &mut TokenCursor` → `iter: &mut Lexer` (44 functions). Also any `cur: &mut TokenCursor` → match the local name (the explorer found `parse_cursor` used `cur`; rename that local to `iter` first, or replace `cur.` accordingly).
(b) Call-site rename (exact strings, safe because the cursor is always `iter`):
```bash
cd crates/huck-syntax/src
sed -i 's/iter\.peek2()/iter.peek2_kind()/g; s/iter\.peek()/iter.peek_kind()/g; s/iter\.next()/iter.next_kind()/g' command.rs
```
(c) `iter.len()` → `iter.remaining()` if present in command.rs; `iter.current_line()`/`iter.peek_span()` unchanged (already methods on `Lexer`).
(d) `parse_pipeline_with_first`, `parse_simple_stage`, `parse_next_stage`, `parse_command_inner` keep `prefix_tokens: Vec<TokenKind>` as-is (Task spec §Mechanics #2 — bounded re-synthesis, not a token source).

- [ ] **Step 4: Update the `source`/`-c` loop in `builtins.rs`**

At `builtins.rs:6412`, `let mut iter = crate::command::TokenCursor::new(tokens);` → `let mut iter = crate::lexer::Lexer::from_tokens(tokens);`. The three `iter.peek_span().map(|sp| sp.offset)` reads (≈6452/6460/6571) are unchanged (`peek_span` exists on `Lexer`). `iter.len() == 0` (≈6461, `consumed_all`) → `iter.remaining() == 0`. `parse_one_unit(&mut iter)` is unchanged.

- [ ] **Step 5: Build + verify byte-identical**

Run: `cargo build --workspace 2>&1 | grep -E "error|warning" | head` (expect none). Then:
Run: `cargo test --workspace 2>&1 | grep "test result" | awk '{p+=$3;f+=$5} END{print "pass",p,"fail",f}'`
Expected: same pass count as baseline (3872), 0 fail.

- [ ] **Step 6: Release harness sweep + commit**

Run: `cargo build -q --release && HUCK_BIN=$(pwd)/target/release/huck bash -c 'for s in tests/scripts/*_diff_check.sh; do timeout 60 bash "$s" >/dev/null 2>&1 || echo "FAIL $s"; done'`
Expected: no FAIL lines.
```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/errors.rs crates/huck-engine/src/builtins.rs
git commit -m "v239 T2: parser + source loop drive &mut Lexer (replay); delete TokenCursor; ParseError::Lex

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Lexer alias storage + command-position expansion

Add the alias map and the command-position pull methods to the `Lexer`, porting the `Expander`'s expansion mechanics (recursion guard, trailing-blank, quoted-skip, body tokenize, span inheritance). The parser does not call these yet (added Task 4) — so behavior is unchanged; new methods are exercised by direct unit tests.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (struct fields, `new`/`from_tokens`, new methods)
- Reference (read only): `crates/huck-engine/src/alias_expand.rs` (mechanics to port: `expand_alias` 216-238, `feed_word` 159-176, `simple_word_text` 331)
- Test: `lexer.rs` tests module

**Interfaces:**
- Produces (on `Lexer`):
  - field `aliases: std::collections::HashMap<String, String>`, `active: std::collections::HashSet<String>`, `cmd_eligible: bool`
  - `pub fn set_aliases(&mut self, aliases: std::collections::HashMap<String, String>)`
  - `pub fn peek_command_kind(&mut self) -> Option<&TokenKind>`
  - `pub fn next_command(&mut self) -> Option<Token>`
  - helper `fn expand_alias_in_place(&mut self, name: &str, body: &str, name_span: Span) -> ()` and `fn alias_candidate(&mut self) -> Option<String>`
  - `Lexer::new`/`from_tokens` gain an `aliases` parameter or default empty map.

- [ ] **Step 1: Add fields + `set_aliases`; thread an aliases arg into the constructors**

Add fields to `struct Lexer`:
```rust
    aliases: std::collections::HashMap<String, String>,
    active: std::collections::HashSet<String>,
    /// True when the next command-position pull should attempt alias expansion
    /// (set on entry and by the trailing-blank rule). Distinct from the parser's
    /// own command-position knowledge: the parser SIGNALS command position by
    /// calling peek_command_kind/next_command; this flag carries the trailing-
    /// blank eligibility forward across one expansion.
    alias_trailing_eligible: bool,
```
Change `fn new(input, opts, brace_expand)` to `fn new(input, opts, brace_expand)` initializing `aliases: HashMap::new(), active: HashSet::new(), alias_trailing_eligible: false`. Same for `from_tokens`. Add:
```rust
pub fn set_aliases(&mut self, aliases: std::collections::HashMap<String, String>) {
    self.aliases = aliases;
}
```

- [ ] **Step 2: Write failing tests for command-position expansion**

```rust
fn lx_with_alias(input: &str, pairs: &[(&str,&str)]) -> Lexer<'static> {
    let toks = tokenize(input).unwrap();
    let mut lx = Lexer::from_tokens(toks);
    let mut m = std::collections::HashMap::new();
    for (k,v) in pairs { m.insert(k.to_string(), v.to_string()); }
    lx.set_aliases(m);
    lx
}

#[test]
fn alias_expands_at_command_position() {
    let mut lx = lx_with_alias("ll /tmp", &[("ll","ls -l")]);
    // command-position pull expands `ll` -> `ls`
    assert_eq!(lx.peek_command_kind().map(word_text_of), Some("ls".into()));
    assert_eq!(lx.next_command().map(|t| word_text_of(&t.kind)), Some("ls".into()));
    // remaining tokens come from the expansion then the original stream
    assert_eq!(lx.next_kind().map(|k| word_text_of(&k)), Some("-l".into()));
    assert_eq!(lx.next_kind().map(|k| word_text_of(&k)), Some("/tmp".into()));
}

#[test]
fn alias_not_expanded_at_argument_position() {
    let mut lx = lx_with_alias("echo ll", &[("ll","ls -l")]);
    assert_eq!(lx.next_command().map(|t| word_text_of(&t.kind)), Some("echo".into()));
    // plain pull at arg position does NOT expand
    assert_eq!(lx.next_kind().map(|k| word_text_of(&k)), Some("ll".into()));
}

#[test]
fn alias_recursion_guard_terminates() {
    let mut lx = lx_with_alias("ls", &[("ls","ls -a")]);
    assert_eq!(lx.next_command().map(|t| word_text_of(&t.kind)), Some("ls".into())); // body's `ls` not re-expanded
    assert_eq!(lx.next_kind().map(|k| word_text_of(&k)), Some("-a".into()));
}

#[test]
fn alias_trailing_blank_makes_next_word_eligible() {
    let mut lx = lx_with_alias("a c", &[("a","b "), ("c","d")]);
    assert_eq!(lx.next_command().map(|t| word_text_of(&t.kind)), Some("b".into()));
    // `a` body ended in blank → next word `c` is command-position eligible
    assert_eq!(lx.next_command().map(|t| word_text_of(&t.kind)), Some("d".into()));
}

#[test]
fn quoted_word_not_expanded() {
    let mut lx = lx_with_alias("'ll'", &[("ll","ls")]);
    // a quoted word never matches an alias
    assert_eq!(lx.peek_command_kind().is_some(), true);
    assert_eq!(lx.next_command().map(|t| word_text_of(&t.kind)), Some("ll".into()));
}
```
Add a `word_text_of(&TokenKind) -> String` test helper if not present (mirror `simple_word_text` for `TokenKind::Word`).

- [ ] **Step 3: Run — verify fail** (`peek_command_kind`/`next_command` undefined).

Run: `cargo test -p huck-syntax alias_ 2>&1 | head`

- [ ] **Step 4: Implement command-position expansion**

Port from `alias_expand.rs`. Add a literal-text helper mirroring `simple_word_text` (returns `None` for quoted/non-literal words):
```rust
fn word_literal_text(w: &Word) -> Option<String> {
    let mut s = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text, quoted: false } => s.push_str(text),
            _ => return None,
        }
    }
    if s.is_empty() { None } else { Some(s) }
}
```
Then the command-position pull (expand at most once per call; idempotent because the result is buffered into `history`):
```rust
/// Expand a registered alias at command position, splicing its body tokens into
/// `history` ahead of `pos`. Mirrors Expander::expand_alias (recursion guard,
/// trailing-blank, span inheritance). Body tokens take the alias-name span.
fn maybe_expand_command_alias(&mut self) {
    // Ensure the upcoming token is materialized.
    self.fill_to(self.pos);
    let Some(tok) = self.history.get(self.pos) else { return };
    let TokenKind::Word(w) = &tok.kind else { return };
    let Some(name) = word_literal_text(w) else { return };
    if self.active.contains(&name) { return; }
    let Some(body) = self.aliases.get(&name).cloned() else { return };
    let name_span = tok.span;
    // Lex the body; on error, stash and stop (the pull then reports EOF).
    let body_tokens = match tokenize(&body) {
        Ok(t) => t,
        Err(e) => { self.pending_error = Some(e); return; }
    };
    // Replace history[pos] (the alias name) with the body tokens, each carrying
    // the alias-name span (so $LINENO points at the use site).
    self.history.remove(self.pos);
    let mut insert_at = self.pos;
    for bt in body_tokens {
        self.history.insert(insert_at, Token::new(bt.kind, name_span));
        insert_at += 1;
    }
    // Recursion guard for the body's own command-position word: re-enter with
    // `name` marked active, expanding the first body token if it is itself an
    // alias (but not `name`).
    self.active.insert(name.clone());
    self.maybe_expand_command_alias();
    self.active.remove(&name);
    // Trailing-blank: a body ending in whitespace makes the NEXT word eligible.
    self.alias_trailing_eligible = body.chars().last().is_some_and(|c| c.is_whitespace());
}

pub fn peek_command_kind(&mut self) -> Option<&TokenKind> {
    self.maybe_expand_command_alias();
    self.peek_kind()
}
pub fn next_command(&mut self) -> Option<Token> {
    self.maybe_expand_command_alias();
    self.next()
}
```
Carry the trailing-blank flag: at the top of `maybe_expand_command_alias`, if `self.alias_trailing_eligible` is set, clear it but still attempt expansion of the now-current word (it is the "next word" made eligible). For the common case the parser only calls the `*_command` variants at real command position, so plain `peek_kind`/`next_kind` between never expand. The trailing-blank flag bridges the one case where an alias body's trailing space makes the immediately following *argument-position* word command-eligible (bash rule).

NOTE for the implementer: verify the recursion/`active` semantics against `alias_expand.rs:216-238` exactly — insert name into `active` BEFORE expanding the body, remove AFTER, so a self-referential alias expands once. The direct tests above pin this.

- [ ] **Step 5: Run — verify pass.** `cargo test -p huck-syntax alias_ 2>&1 | grep "test result"` (expect all pass). Then full crate: `cargo test -p huck-syntax 2>&1 | grep "test result"`.

- [ ] **Step 6: Workspace sanity (no behavior change — methods unused) + commit**

Run: `cargo test --workspace 2>&1 | grep "test result" | awk '{p+=$3;f+=$5} END{print p,f}'` (3872, 0).
```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v239 T3: Lexer alias storage + command-position expansion (ported from Expander)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Parser calls command-position methods; REPL goes live; other callers adapt to `&mut Lexer`

Hook the two command-position sites to `peek_command_kind`/`next_command`, change `parse` to take `&mut Lexer`, and make the **REPL** build a live `Lexer` with the alias map (read-time alias). The other `parse` callers — continuation, command-substitution bodies, function reconstruction, prompt cmdsub — are **alias-free today** (none call `expand_aliases_in_tokens`), so they must stay byte-identical: keep `tokenize` then wrap the vec in `Lexer::from_tokens` (replay, no aliases) and call `parse(&mut lx)`. **Do NOT pass the alias map to command-substitution bodies** — current huck does not expand aliases inside `$(...)`, and doing so would diverge. The `source` loop goes live in Task 5.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (sites 1115, 2381; `parse` signature)
- Modify: `crates/huck-engine/src/shell.rs` (REPL ≈380-401), `continuation.rs` (≈50-77), `crates/huck-syntax/src/lexer.rs` (`parse_substitution_body` ≈2876), `shell_state.rs` (≈687), `prompt.rs` (≈249)

**Interfaces:**
- Consumes: `Lexer::new` (now takes/initializes aliases; add a `Lexer::new_live(input, &aliases, opts) -> Lexer` convenience that sets the map), `peek_command_kind`, `next_command`, `set_aliases`, `take_error`, `from_tokens`.
- Produces: `pub fn parse(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError>` (signature change — `Vec<Token>` callers now wrap in `Lexer::from_tokens`).

- [ ] **Step 1: Hook the two command-position sites**

In `parse_command_inner` (`command.rs:1111-1115`):
```rust
if matches!(iter.peek_command_kind(), Some(TokenKind::Word(_))) {
    let word_line = iter.current_line();
    let Some(TokenKind::Word(w)) = iter.next_command().map(|t| t.kind) else { unreachable!() };
```
In `parse_next_stage` (`command.rs:2378-2381`): same change (`peek_command_kind` / `next_command().map(|t| t.kind)`). These are the ONLY two sites that use the `*_command` variants.

- [ ] **Step 2: Change `parse` signature; expose `Lexer::new_live`**

In `lexer.rs`:
```rust
pub fn new_live(input: &'a str, aliases: &std::collections::HashMap<String, String>, opts: LexerOptions) -> Lexer<'a> {
    let mut lx = Lexer::new(input, opts, true);
    lx.aliases = aliases.clone();
    lx
}
```
In `command.rs`, `parse` becomes a thin wrapper over `parse_cursor` (callers now own the lexer):
```rust
pub fn parse(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    parse_cursor(iter)
}
```
Note: the `Vec<Token>` callers that don't alias-expand (continuation, comsub, function-body, prompt — Step 4) now wrap with `Lexer::from_tokens(...)`; the alias-expanding callers (REPL here, source loop in Task 5) use `new_live`.

- [ ] **Step 3: REPL — build a live lexer (drop the pre-pass)**

In `shell.rs:process_line_in_sinks`, replace the `tokenize_with_opts` + `expand_aliases_in_tokens` + `command::parse(tokens)` block with:
```rust
let opts = lexer::LexerOptions { extglob: shell.extglob(), ..Default::default() };
let empty = std::collections::HashMap::new();
let aliases = if expand_aliases { &shell.aliases } else { &empty };
let mut lx = lexer::Lexer::new_live(line, aliases, opts);
match command::parse(&mut lx) {
    Ok(Some(sequence)) => executor::execute_with_sink(&sequence, shell, line, sink, err_sink),
    Ok(None) => ExecOutcome::Continue(0),
    Err(command::ParseError::Lex(e)) => {
        { let mut err = crate::executor::err_writer(err_sink, sink); e!(&mut *err, "huck: syntax error{}", crate::lex_error_message(&e)); }
        ExecOutcome::Continue(2)
    }
    Err(e) => {
        { let mut err = crate::executor::err_writer(err_sink, sink); e!(&mut *err, "huck: syntax error: {}", crate::parse_error_message(&e)); }
        ExecOutcome::Continue(2)
    }
}
```
(The `Lex` arm reproduces the previous tokenize-error message exactly; verify the format string matches the old `"huck: syntax error{}"` lex path.)

- [ ] **Step 4: continuation, comsub, function-body, prompt — adapt to `&mut Lexer` (replay, no aliases, byte-identical)**

These callers are alias-free today; keep them so. They keep calling `tokenize`/`tokenize_with_opts` (which still exists and surfaces lex errors up front exactly as now) and only swap the `parse(vec)` call for `parse(&mut Lexer::from_tokens(vec))`.

`continuation.rs:classify`: the early lex-error classification block (the `match tokenize_with_opts(...)` arms for `UnterminatedHeredoc`/unterminated quote) is **unchanged**. Only the two `command::parse(...)` calls change:
```rust
if let Err(ParseError::UnterminatedDoubleBracket) =
    command::parse(&mut lexer::Lexer::from_tokens(tokens.clone()))
{
    return Completeness::Incomplete(ContinuationReason::DoubleBracket);
}
// trailing-operator check on `tokens.last()` is UNCHANGED (still has the vec)
...
match command::parse(&mut lexer::Lexer::from_tokens(tokens)) { /* arms unchanged */ }
```

`lexer.rs:parse_substitution_body`: keep tokenizing the body and zeroing inner lines exactly as today; only swap `command::parse(tokens)` for the `&mut Lexer` form. **No alias map** (matches current huck — `$()` bodies are not alias-expanded). Signature unchanged:
```rust
fn parse_substitution_body(body: &str, opts: LexerOptions) -> Result<crate::command::Sequence, LexError> {
    let mut tokens = tokenize_with_opts(body, opts).map_err(|e| LexError::Substitution(Box::new(e)))?;
    for t in &mut tokens { t.span.line = 0; }                 // unchanged: $LINENO isolation
    let mut lx = Lexer::from_tokens(tokens);
    let parsed = crate::command::parse(&mut lx).map_err(LexError::SubstitutionParseError)?;
    Ok(parsed.unwrap_or_else(empty_sequence))
}
```
(The two callers `scan_paren_substitution`/`scan_backtick_substitution` are unchanged — they still call `parse_substitution_body(&body, opts)`.)

`shell_state.rs:687` and `prompt.rs:249`: replace `command::parse(tokens)` with `command::parse(&mut crate::lexer::Lexer::from_tokens(tokens))` (keep the preceding `tokenize(&src)` as-is). No aliases.

- [ ] **Step 5: Build, full suite, harness sweep (REPL now incremental + read-time alias)**

Run: `cargo test --workspace 2>&1 | grep "test result" | awk '{p+=$3;f+=$5} END{print p,f}'` (3872, 0). Then the release harness sweep (Task 2 Step 6 command), `alias*` harnesses included — must be clean.

- [ ] **Step 6: Commit**
```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/lexer.rs crates/huck-engine/src/shell.rs crates/huck-engine/src/continuation.rs crates/huck-engine/src/shell_state.rs crates/huck-engine/src/prompt.rs
git commit -m "v239 T4: parser command-position alias calls; REPL/continuation/comsub/etc. go live

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `source`/`-c` loop live + between-unit `set_aliases`; retire `alias_generation` hack

Drive the source loop with a single live `Lexer` over the chunk, refresh its alias map between units (def-then-use), and remove the `alias_generation` re-tokenize path (no longer needed — the live pull picks up new aliases lazily).

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`run_sourced_contents_in_sinks` 6309-6579)

**Interfaces:**
- Consumes: `Lexer::new_live`, `set_aliases`, `parse_one_unit(&mut Lexer)`, `peek_span`, `remaining`, `take_error`.

- [ ] **Step 1: Replace tokenize_partial+pre-pass+from_tokens with one live lexer**

Replace the per-chunk `tokenize_partial` → `expand_aliases_in_tokens` → `Lexer::from_tokens` (≈6350-6412) with:
```rust
let opts = crate::lexer::LexerOptions { extglob, ..Default::default() };
let empty = std::collections::HashMap::new();
let aliases_now = if expand { shell.aliases.clone() } else { empty.clone() };
let mut iter = crate::lexer::Lexer::new_live(&contents[start..], &aliases_now, opts);
```
The `unit_start_off`/`unit_end_off` offset reads (`iter.peek_span().map(|sp| sp.offset)`) and `iter.remaining()` work unchanged on the live lexer; offsets are chunk-relative as before.

- [ ] **Step 2: Refresh aliases between units; drop the `alias_generation` gate**

After each unit executes (the existing execute call in the loop), before the next `parse_one_unit`, refresh the lexer's aliases if expansion is active:
```rust
if expand {
    iter.set_aliases(shell.aliases.clone());
}
```
Remove the `alias_gen`/`shell.alias_generation` snapshot (6386) and the `'outer` re-tokenize restart condition (6507-6509) — the live lexer already reflects the refreshed map for the next unit. (Keep the `extglob`-change handling if it gates on `new_extglob != extglob`; only the alias-generation clause is removed. If `extglob` can change mid-source, rebuild the lexer's `opts` similarly or accept that `set_extglob` is also needed — verify against the existing extglob-in-source harness.)

- [ ] **Step 3: Build, full suite, harness sweep — def-then-use must work**

Add a bash-diff fragment OR a Rust integration test for cross-unit def-then-use in a sourced file:
```
alias greet='echo hi'
greet
```
must print `hi` (alias defined on a prior line takes effect), while
```
alias greet='echo hi'; greet
```
on one line must error/treat `greet` as a command (same-unit non-expansion).
Run the full suite + harness sweep; `source`/alias harnesses clean.

- [ ] **Step 4: Commit**
```bash
git add crates/huck-engine/src/builtins.rs
git commit -m "v239 T5: source/-c loop live + between-unit set_aliases; retire alias_generation hack

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Retire the alias pre-pass; direct incrementality + error tests; cleanup

Delete the now-unused `huck-engine` alias pre-pass and add the remaining direct tests (parser-level incrementality, bad-alias-body stash, `$(...)`-with-alias).

**Files:**
- Modify/Delete: `crates/huck-engine/src/alias_expand.rs` (delete `expand_aliases_in_tokens` / `_mapped` / `Expander` if no longer referenced; keep `simple_word_text` if used elsewhere — grep first)
- Modify: `crates/huck-engine/src/shell_state.rs` (remove `alias_generation` field if now unreferenced — grep; the `alias`/`unalias` builtins drop the `+= 1` lines)
- Test: `crates/huck-syntax/src/lexer.rs`, `crates/huck-engine` integration tests
- Add: `tests/scripts/alias_readtime_diff_check.sh` (new bash-diff harness)

**Interfaces:** none new.

- [ ] **Step 1: Grep for remaining references, then delete the pre-pass**

Run: `rg 'expand_aliases_in_tokens|alias_generation|Expander' crates/`
Remove every now-dead path: the `alias_expand` pre-pass entry points, the `alias_generation` field (`shell_state.rs:428`) and its `+= 1` bumps (`builtins.rs:6633/6656/6665`) if nothing else reads it. Keep `simple_word_text` only if still referenced (the lexer port has its own `word_literal_text`).

- [ ] **Step 2: Direct incrementality + stash tests (lexer.rs)**

```rust
#[test]
fn parser_pull_is_incremental_not_batch() {
    // A live lexer over many commands: after parsing one unit, the cursor must
    // not have scanned the whole input.
    let input = (0..50).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("\n");
    let empty = std::collections::HashMap::new();
    let mut lx = Lexer::new_live(&input, &empty, LexerOptions::default());
    let _ = crate::command::parse_one_unit(&mut lx).unwrap();
    // history holds far fewer than all tokens (only the first unit + lookahead).
    assert!(lx.scanned_token_count() < 10, "scanned too much: not incremental");
}

#[test]
fn bad_alias_body_surfaces_as_parse_error() {
    let mut m = std::collections::HashMap::new();
    m.insert("x".to_string(), "echo \"".to_string());  // unterminated quote in body
    let mut lx = Lexer::new_live("x", &m, LexerOptions::default());
    let r = crate::command::parse(&mut lx);
    assert!(matches!(r, Err(crate::command::ParseError::Lex(_))));
}
```
Add a `#[cfg(test)] pub fn scanned_token_count(&self) -> usize { self.history.len() }` accessor on `Lexer`.

- [ ] **Step 3: `$(...)`-with-alias + new bash-diff harness**

Create `tests/scripts/alias_readtime_diff_check.sh` modeled on the existing `alias*_diff_check.sh` (same PASS/FAIL harness shape), covering: command-position vs argument-position; recursion; trailing-blank; `alias` to a reserved word (`alias x='if'`); cross-unit def-then-use; alias expansion inside `$(...)`; quoted word not expanded. Each fragment run through bash and `target/release/huck`, asserting identical output.

- [ ] **Step 4: Full suite + ALL harnesses (incl. the new one)**

Run: `cargo test --workspace 2>&1 | grep "test result" | awk '{p+=$3;f+=$5} END{print p,f}'` (≥3877, 0).
Run the release harness sweep over `tests/scripts/*_diff_check.sh` (now 153 scripts) — all clean.

- [ ] **Step 5: Commit**
```bash
git add -A
git commit -m "v239 T6: retire alias pre-pass + alias_generation; incrementality/stash tests; alias_readtime harness

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **Byte-identical is the law.** After each task run the full `cargo test --workspace` and (Tasks 2/4/5/6) the release harness sweep. A red harness is a stop-and-fix, not a "close enough".
- **The `alias*` harnesses are the alias oracle.** If one changes output, the port of the `Expander` mechanics is wrong — re-check `alias_expand.rs:216-238` (recursion `active` ordering) and `feed_word` eligibility (`alias_expand.rs:159-176`).
- **Incremental error ordering** (spec Risks): a line with both an early parse error and a later lex error now reports the parse error first (bash-aligned). If a harness pins the old batch order, treat it as an intentional change and note it in the merge.
- **`$LINENO` through aliases / `$()`**: body tokens inherit the alias-name span (Task 3); inner `$()` lines are zeroed (Task 4). The `lineno*`/`comsub*` harnesses pin this.
