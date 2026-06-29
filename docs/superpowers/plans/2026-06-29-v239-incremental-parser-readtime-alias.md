# v239 — Incremental parser + read-time alias in the lexer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the parser pull tokens **live** from the `Lexer` one at a time (no up-front `Vec`), and fold alias expansion into the lexer's command-position pull, retiring the `huck-engine` alias pre-pass, the `alias_generation` re-tokenize hack, and `TokenCursor`.

**Architecture:** The `Lexer` (already incremental via v238's `next_token`) gains a parser-facing pull API (`peek`/`next` yielding whole `Token`s, plus `*_kind` convenience methods, `peek_command_kind`/`next_command`, `peek_span`/`current_line`/`remaining`, `set_aliases`). The parser holds `&mut Lexer` and drives it; at the two command-position sites it calls the `*_command` variants and the lexer expands a registered alias in place. **The pull is fallible: every pull method returns `Result<_, LexError>`** — a lex error hit while scanning is *returned at the failing call*, not stashed on the lexer. `impl From<LexError> for ParseError` lets each parser site propagate with `?`.

**Tech Stack:** Rust. Crates `huck-syntax` (`lexer.rs`, `command.rs`, `errors.rs`) and `huck-engine` (`shell.rs`, `builtins.rs`, `continuation.rs`, `shell_state.rs`, `prompt.rs`, `alias_expand.rs`).

**Spec:** `docs/superpowers/specs/2026-06-29-v239-incremental-parser-readtime-alias-design.md`

## Global Constraints

- **Byte-identical output** for every input — same `Sequence` AST, same errors, same exit codes. Oracle: `cargo test --workspace` (currently ~3874 tests) + all 152 `tests/scripts/*_diff_check.sh` harnesses (release binary, `HUCK_BIN=target/release/huck`), the `alias*` harnesses the critical lens.
- **Run the FULL suite:** `cargo test --workspace` (plain `cargo test` skips most crates). 0 warnings.
- **Fallible pull, `?`-propagated (NO error stash).** Every scanning pull method returns `Result<Option<…>, LexError>` (`remaining` is the only non-scanning exception). A lex error surfaces at the call that hit it. `impl From<LexError> for ParseError` (it wraps `Lex(Box<LexError>)`) makes every parser site `iter.peek_kind()?`. There is NO `pending_error` field and NO `take_error` — the v239-T1 stash is removed in Task 2.
- **`Box<LexError>` in `ParseError::Lex` is an intentional stopgap, not a defect.** `LexError` already contains a `ParseError` (`SubstitutionParseError`, from `parse_substitution_body` calling `command::parse`), so `ParseError::Lex(LexError)` would be a recursive type (E0072). The clean fix (remove the lexer→parser edge) is deferred to a later round (Phase C of the re-arch). Keep the `Box`; do NOT un-box it; add NO new lexer→parser calls.
- **Primary pull yields whole `Token`s** (kind + span); `*_kind` convenience methods serve the kind-only sites so the parser change there is `iter.peek()`→`iter.peek_kind()?` — a near-mechanical rename plus a `?`.
- **Alias rules preserved verbatim** from `Expander`: recursion guard (`active` set), trailing-blank eligibility (`body.chars().last().is_whitespace()`), quoted/non-literal words never match (`simple_word_text` returns `None`), alias body tokens inherit the alias-name span/src for `$LINENO`, expansion happens before the parser's reserved-word check.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Branch `v239-incremental-parser` (already created from `main`).

## File map

- `crates/huck-syntax/src/lexer.rs` — `Lexer` struct (`pub`), fallible pull API, `from_tokens`, alias storage + command-position expansion, `parse_substitution_body` adaptation.
- `crates/huck-syntax/src/command.rs` — `ParseError::Lex(Box<LexError>)` + `From<LexError>`, parser drives `&mut Lexer` with `?`, delete `TokenCursor`, command-position calls.
- `crates/huck-syntax/src/errors.rs` — `ParseError::Lex` message arm.
- `crates/huck-engine/src/shell.rs` — REPL builds a live `Lexer`.
- `crates/huck-engine/src/builtins.rs` — `source`/`-c` loop drives a `Lexer`, between-unit `set_aliases`, retire `alias_generation` gate.
- `crates/huck-engine/src/continuation.rs`, `shell_state.rs`, `prompt.rs` — adapt to `parse(&mut Lexer)` via `from_tokens`.
- `crates/huck-engine/src/alias_expand.rs` — pre-pass entry points deleted (Task 7); expansion mechanics referenced when porting (Task 4).

---

### Task 1: Lexer parser-facing pull API + replay constructor — **SHIPPED (commit 26c69f6)**

Task 1 is complete and merged on the branch. It added `pub struct Lexer`, `from_tokens` (replay), `fill_to`, and the pull methods — **but returning `Option` with a `pending_error`/`take_error` stash.** That error model is superseded: **Task 2 converts the pull to `Result` and deletes the stash.** Do not re-run Task 1; start at Task 2.

---

### Task 2: Result-ize the lexer pull API (delete the `pending_error`/`take_error` stash)

Task 1's pull returns `Option` and parks lex errors in `pending_error`, retrieved via `take_error`. A side-channel error flag is a hack: an error hit while scanning must be **returned**. Convert every scanning pull method to `Result<_, LexError>`, make `fill_to` propagate `scan_step`'s error with `?`, and delete `pending_error`/`take_error`. This touches only `lexer.rs`; nothing calls these methods yet except the Task 1 unit tests, so it is byte-identical.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (the pull methods + `fill_to` + struct field added in Task 1)
- Test: `crates/huck-syntax/src/lexer.rs` (update the 2 Task 1 pull tests to the Result API)

**Interfaces:**
- Consumes: Task 1's `pub struct Lexer`, `from_tokens`, `replay` field, `scan_step() -> Result<Step, LexError>`, `Step`, `backfill_pending_at`.
- Produces (replace the Option-returning versions in place):
  - `pub fn peek(&mut self) -> Result<Option<&Token>, LexError>`
  - `pub fn next(&mut self) -> Result<Option<Token>, LexError>`
  - `pub fn peek_kind(&mut self) -> Result<Option<&TokenKind>, LexError>`
  - `pub fn peek2_kind(&mut self) -> Result<Option<&TokenKind>, LexError>`
  - `pub fn next_kind(&mut self) -> Result<Option<TokenKind>, LexError>`
  - `pub fn peek_span(&mut self) -> Result<Option<Span>, LexError>`
  - `pub fn current_line(&mut self) -> Result<u32, LexError>`
  - `pub fn remaining(&self) -> usize` (UNCHANGED — never scans)
  - `fn fill_to(&mut self, idx: usize) -> Result<(), LexError>` (private)
  - REMOVED: the `pending_error` field and `pub fn take_error`. (The `replay` field STAYS.)

- [ ] **Step 1: Update the 2 Task 1 pull tests to the Result API**

In the `lexer.rs` test module, change the existing `pull_api_reproduces_token_sequence` / `pull_next_kind_matches_next_dot_kind` tests to the Result shape (and drop the `take_error` assertion):
```rust
#[test]
fn pull_api_reproduces_token_sequence() {
    let toks = tokenize("echo foo | grep bar").unwrap();
    let mut lx = Lexer::from_tokens(toks.clone());
    assert_eq!(lx.remaining(), toks.len());
    assert_eq!(lx.peek_kind().unwrap(), Some(&toks[0].kind));
    assert_eq!(lx.peek2_kind().unwrap(), Some(&toks[1].kind));
    assert_eq!(lx.peek_span().unwrap(), Some(toks[0].span));
    let mut drained = Vec::new();
    while let Some(t) = lx.next().unwrap() { drained.push(t); }
    assert_eq!(drained, toks);
    assert_eq!(lx.peek_kind().unwrap(), None);
    assert_eq!(lx.next_kind().unwrap(), None);
}

#[test]
fn pull_next_kind_matches_next_dot_kind() {
    let toks = tokenize("a b c").unwrap();
    let mut lx = Lexer::from_tokens(toks.clone());
    assert_eq!(lx.next_kind().unwrap(), Some(toks[0].kind.clone()));
    assert_eq!(lx.peek_kind().unwrap(), Some(&toks[1].kind));
}

#[test]
fn pull_surfaces_lex_error_as_err() {
    // A genuinely unterminated construct: the pull returns Err at the failing scan.
    let mut lx = Lexer::new("echo \"unterminated", LexerOptions::default(), true);
    // drain until we hit the error
    let mut got_err = false;
    loop {
        match lx.next() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => { got_err = true; break; }
        }
    }
    assert!(got_err, "unterminated quote must surface as Err from the pull");
}
```
(If `Lexer::new`'s exact signature differs, match it — it is `fn new(input, opts, brace_expand)`. Pick any input the batch `tokenize` already rejects with a `LexError` so the oracle is the existing lexer behavior.)

- [ ] **Step 2: Run tests — verify they fail to compile** (`Result` vs `Option` mismatch / `take_error` gone)

Run: `cargo test -p huck-syntax pull_ 2>&1 | head`
Expected: type errors.

- [ ] **Step 3: Remove the stash and convert the methods**

Delete the `pending_error: Option<LexError>` field from `struct Lexer` and from BOTH initializers (`Lexer::new` and `Lexer::from_tokens`). Delete `pub fn take_error`. Convert `fill_to` and the pull methods:
```rust
/// Ensure history[idx] exists AND is backfill-ready (heredoc body present),
/// pulling lazily via scan_step. Mirrors next_token's readiness rule so a
/// Heredoc token is never exposed before its body is collected (v238). On a lex
/// error, RETURN it (no stash). scan_step appends to history without advancing pos.
fn fill_to(&mut self, idx: usize) -> Result<(), LexError> {
    if self.replay {
        return Ok(());
    }
    loop {
        if self.history.len() > idx && !self.backfill_pending_at(idx) {
            return Ok(());
        }
        match self.scan_step()? {
            Step::Produced => {}
            Step::Eof => return Ok(()),
        }
    }
}

pub fn peek(&mut self) -> Result<Option<&Token>, LexError> {
    self.fill_to(self.pos)?;
    Ok(self.history.get(self.pos))
}
pub fn next(&mut self) -> Result<Option<Token>, LexError> {
    self.fill_to(self.pos)?;
    let t = self.history.get(self.pos).cloned();
    if t.is_some() { self.pos += 1; }
    Ok(t)
}
pub fn peek_kind(&mut self) -> Result<Option<&TokenKind>, LexError> {
    self.fill_to(self.pos)?;
    Ok(self.history.get(self.pos).map(|t| &t.kind))
}
pub fn peek2_kind(&mut self) -> Result<Option<&TokenKind>, LexError> {
    self.fill_to(self.pos + 1)?;
    Ok(self.history.get(self.pos + 1).map(|t| &t.kind))
}
pub fn next_kind(&mut self) -> Result<Option<TokenKind>, LexError> {
    Ok(self.next()?.map(|t| t.kind))
}
pub fn peek_span(&mut self) -> Result<Option<Span>, LexError> {
    self.fill_to(self.pos)?;
    Ok(self.history.get(self.pos).map(|t| t.span))
}
pub fn current_line(&mut self) -> Result<u32, LexError> {
    Ok(self.peek_span()?.map(|s| s.line).unwrap_or(0))
}
pub fn remaining(&self) -> usize {
    self.history.len().saturating_sub(self.pos)
}
```

- [ ] **Step 4: Run tests + crate — verify pass, 0 warnings**

Run: `cargo test -p huck-syntax pull_ 2>&1 | grep "test result"` (all pass), then `cargo test -p huck-syntax 2>&1 | grep "test result"` and `cargo build -p huck-syntax 2>&1 | grep -c warning` (→ 0).

- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v239 T2: Result-ize lexer pull API; drop pending_error/take_error stash

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Flip the parser + source loop to drive `&mut Lexer` (`?`-propagated); delete `TokenCursor`

Add `ParseError::Lex(Box<LexError>)` + `impl From<LexError> for ParseError`, so every parser pull site is `iter.peek_kind()?` — the `?` converts `LexError`→`ParseError`. Rewrite `parse_cursor`/`parse_one_unit` (errors propagate via `?`, no stash check). Sweep the ~150 sites. Delete `TokenCursor`. `parse(tokens)` keeps its `Vec<Token>` signature (builds `from_tokens` internally) so the other callers are untouched this task; the `source` loop swaps `TokenCursor::new` → `Lexer::from_tokens`. The alias pre-pass still runs → **byte-identical**.

**Cascade note (the main work beyond the sed):** a few parser helpers that pull tokens currently return `bool`/`()` (e.g. `skip_newlines`). With a fallible pull they must become `Result<_, ParseError>` and their callers add `?`. The compiler enumerates these; it is bounded and mechanical. Do NOT swallow a pull error with `.unwrap()`/`.ok()` to dodge the cascade.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (`TokenCursor` 773-817, `parse_cursor` 819, `parse` 833, `parse_one_unit` 897, `ParseError` 731-758, all 44 `&mut TokenCursor` fns, ~150 pull sites)
- Modify: `crates/huck-syntax/src/errors.rs` (parse-error message arm)
- Modify: `crates/huck-engine/src/builtins.rs` (source loop ≈6412 `TokenCursor::new`, ≈6452/6460/6571 `peek_span`, `iter.len()` ≈6461)

**Interfaces:**
- Consumes (from Task 2): `Lexer::from_tokens`, and the fallible `peek`/`next`/`peek_kind`/`peek2_kind`/`next_kind`/`peek_span`/`current_line`/`remaining`. `Lexer` is `pub`.
- Produces: `pub enum ParseError { …, Lex(Box<crate::lexer::LexError>) }`; `impl From<crate::lexer::LexError> for ParseError`; `pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError>` (unchanged signature, builds `from_tokens`); `pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError>`. `TokenCursor` removed.

- [ ] **Step 1: Add `ParseError::Lex` + `From<LexError>` + message arm**

In `command.rs`, add to `enum ParseError` (after `ArithForHeader(String),`):
```rust
    /// NEW (v239): a lex error surfaced while the parser pulled tokens live
    /// (e.g. a bad alias body). Boxed because `LexError` already contains a
    /// `ParseError` (`SubstitutionParseError`), so an unboxed variant would be a
    /// recursive type with infinite size (E0072). Box is an intentional stopgap.
    Lex(Box<crate::lexer::LexError>),
```
And the conversion that powers `?`:
```rust
impl From<crate::lexer::LexError> for ParseError {
    fn from(e: crate::lexer::LexError) -> Self {
        ParseError::Lex(Box::new(e))
    }
}
```
In `crates/huck-syntax/src/errors.rs`, find `parse_error_message_impl` and add an arm (the `Box<LexError>` derefs to `&LexError` automatically):
```rust
ParseError::Lex(e) => crate::lex_error_message(e),
```
(Use whatever lex-error message helper `errors.rs` already exposes — `lex_error_message`/`lex_error_message_impl` — so the text matches today's output.)

- [ ] **Step 2: Delete `TokenCursor`; rewrite `parse`/`parse_cursor`/`parse_one_unit`**

Delete `struct TokenCursor` + its `impl` + `impl Iterator` + `impl ExactSizeIterator` (command.rs:773-817). Rewrite the entry functions — errors flow through `?`, no `take_error`:
```rust
fn parse_cursor(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let seq = parse_sequence(iter, &[])?;
    if iter.peek_kind()?.is_some() {
        return Err(ParseError::UnexpectedToken);
    }
    Ok(Some(seq))
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    let mut iter = Lexer::from_tokens(tokens);
    parse_cursor(&mut iter)
}

pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::Newline)) {
        iter.next_kind()?;
    }
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let seq = parse_sequence_opts(iter, &[], true)?;
    Ok(Some(seq))
}
```
Add `use crate::lexer::Lexer;` at the top of `command.rs` if not present. (`skip_newlines` becomes `Result<(), ParseError>` per the cascade — Step 3.)

- [ ] **Step 3: Rename + `?`-suffix the pull sites across `command.rs`**

(a) Parameter type: every `iter: &mut TokenCursor` → `iter: &mut Lexer` (44 functions). If `parse_cursor` (or any fn) used a different local name (`cur`), normalize it to `iter` first.
(b) Call-site sweep (exact strings; the cursor is always `iter`):
```bash
cd crates/huck-syntax/src
sed -i 's/iter\.peek2()/iter.peek2_kind()?/g; s/iter\.peek()/iter.peek_kind()?/g; s/iter\.next()/iter.next_kind()?/g' command.rs
sed -i 's/iter\.current_line()/iter.current_line()?/g; s/iter\.peek_span()/iter.peek_span()?/g' command.rs
sed -i 's/iter\.len()/iter.remaining()/g' command.rs
```
(c) **Compile and resolve the cascade.** `cargo build -p huck-syntax` will flag: (i) any non-cursor `iter` (a real Rust iterator) wrongly given `?`/`_kind` — revert those specific sites to `.next()`/`.peek()`; (ii) helpers that pull but return non-`Result` — change their return type to `Result<_, ParseError>` and add `?` at their callers. Iterate until it builds. Do not `.unwrap()` a pull to silence a cascade error.
(d) `parse_pipeline_with_first`, `parse_simple_stage`, `parse_next_stage`, `parse_command_inner` keep their `prefix_tokens: Vec<TokenKind>` peel as-is (bounded re-synthesis, not a token source).

- [ ] **Step 4: Update the `source`/`-c` loop in `builtins.rs`**

At `builtins.rs:6412`, `let mut iter = crate::command::TokenCursor::new(tokens);` → `let mut iter = crate::lexer::Lexer::from_tokens(tokens);`. `peek_span` now returns `Result` and this fn does not return `ParseError`, so read it explicitly (a replay lexer never errors; this also stays correct when the loop goes live in Task 6):
- the three `iter.peek_span().map(|sp| sp.offset)` reads (≈6452/6460/6571) → `iter.peek_span().ok().flatten().map(|sp| sp.offset)`.
- `iter.len() == 0` (≈6461, `consumed_all`) → `iter.remaining() == 0`.
- `parse_one_unit(&mut iter)` already returns `Result<_, ParseError>` and is handled as today.

- [ ] **Step 5: Build + verify byte-identical**

Run: `cargo build --workspace 2>&1 | grep -E "error|warning" | head` (expect none). Then
Run: `cargo test --workspace 2>&1 | grep "test result" | awk '{p+=$3;f+=$5} END{print "pass",p,"fail",f}'`
Expected: baseline pass count (~3874), 0 fail.

- [ ] **Step 6: Release harness sweep + commit**

Run: `cargo build -q --release && HUCK_BIN=$(pwd)/target/release/huck bash -c 'for s in tests/scripts/*_diff_check.sh; do timeout 60 bash "$s" >/dev/null 2>&1 || echo "FAIL $s"; done'`
Expected: no FAIL lines.
```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/errors.rs crates/huck-engine/src/builtins.rs
git commit -m "v239 T3: parser + source loop drive &mut Lexer (?-propagated); delete TokenCursor; ParseError::Lex

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Lexer alias storage + command-position expansion

Add the alias map and the command-position pull methods to the `Lexer`, porting the `Expander`'s expansion mechanics (recursion guard, trailing-blank, quoted-skip, body tokenize, span inheritance). The parser does not call these yet (Task 5) — so behavior is unchanged; new methods are exercised by direct unit tests. The command-position methods are **fallible** (the alias body is lexed; a bad body returns `Err(LexError)`).

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (struct fields, `new`/`from_tokens` initializers, new methods)
- Reference (read only): `crates/huck-engine/src/alias_expand.rs` (mechanics to port: `expand_alias` 216-238, `feed_word` 159-176, `simple_word_text` 331)
- Test: `lexer.rs` tests module

**Interfaces:**
- Produces (on `Lexer`):
  - fields `aliases: HashMap<String,String>`, `active: HashSet<String>`, `alias_trailing_eligible: bool`
  - `pub fn set_aliases(&mut self, aliases: HashMap<String, String>)`
  - `pub fn peek_command_kind(&mut self) -> Result<Option<&TokenKind>, LexError>`
  - `pub fn next_command(&mut self) -> Result<Option<Token>, LexError>`
  - helpers `fn maybe_expand_command_alias(&mut self) -> Result<(), LexError>`, `fn word_literal_text(w: &Word) -> Option<String>`

- [ ] **Step 1: Add fields + `set_aliases`; initialize in both constructors**

Add to `struct Lexer`:
```rust
    aliases: std::collections::HashMap<String, String>,
    active: std::collections::HashSet<String>,
    /// Carries bash's trailing-blank rule across one expansion: a body ending in
    /// whitespace makes the NEXT word command-position eligible.
    alias_trailing_eligible: bool,
```
Initialize `aliases: HashMap::new(), active: HashSet::new(), alias_trailing_eligible: false` in BOTH `Lexer::new` and `Lexer::from_tokens`. Add:
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
fn wtext(k: &TokenKind) -> String {
    if let TokenKind::Word(w) = k { word_literal_text(w).unwrap_or_default() } else { String::new() }
}

#[test]
fn alias_expands_at_command_position() {
    let mut lx = lx_with_alias("ll /tmp", &[("ll","ls -l")]);
    assert_eq!(lx.peek_command_kind().unwrap().map(wtext), Some("ls".into()));
    assert_eq!(lx.next_command().unwrap().map(|t| wtext(&t.kind)), Some("ls".into()));
    assert_eq!(lx.next_kind().unwrap().map(|k| wtext(&k)), Some("-l".into()));
    assert_eq!(lx.next_kind().unwrap().map(|k| wtext(&k)), Some("/tmp".into()));
}

#[test]
fn alias_not_expanded_at_argument_position() {
    let mut lx = lx_with_alias("echo ll", &[("ll","ls -l")]);
    assert_eq!(lx.next_command().unwrap().map(|t| wtext(&t.kind)), Some("echo".into()));
    assert_eq!(lx.next_kind().unwrap().map(|k| wtext(&k)), Some("ll".into()));
}

#[test]
fn alias_recursion_guard_terminates() {
    let mut lx = lx_with_alias("ls", &[("ls","ls -a")]);
    assert_eq!(lx.next_command().unwrap().map(|t| wtext(&t.kind)), Some("ls".into()));
    assert_eq!(lx.next_kind().unwrap().map(|k| wtext(&k)), Some("-a".into()));
}

#[test]
fn alias_trailing_blank_makes_next_word_eligible() {
    let mut lx = lx_with_alias("a c", &[("a","b "), ("c","d")]);
    assert_eq!(lx.next_command().unwrap().map(|t| wtext(&t.kind)), Some("b".into()));
    assert_eq!(lx.next_command().unwrap().map(|t| wtext(&t.kind)), Some("d".into()));
}

#[test]
fn quoted_word_not_expanded() {
    let mut lx = lx_with_alias("'ll'", &[("ll","ls")]);
    assert_eq!(lx.next_command().unwrap().map(|t| wtext(&t.kind)), Some("ll".into()));
}

#[test]
fn bad_alias_body_returns_err() {
    let mut lx = lx_with_alias("x", &[("x","echo \"")]); // unterminated quote in body
    assert!(lx.next_command().is_err());
}
```

- [ ] **Step 3: Run — verify fail** (`peek_command_kind`/`next_command`/`word_literal_text` undefined).

Run: `cargo test -p huck-syntax alias_ bad_alias quoted_word 2>&1 | head`

- [ ] **Step 4: Implement command-position expansion (port from `alias_expand.rs`)**

Literal-text helper (mirrors `simple_word_text`: `None` for quoted/non-literal — so those never match an alias):
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
Expansion (fallible — propagate the body's lex error, do NOT stash):
```rust
/// Expand a registered alias at command position by splicing its body tokens into
/// `history` ahead of `pos`. Mirrors Expander::expand_alias (recursion guard,
/// trailing-blank, span inheritance). Body tokens take the alias-name span.
fn maybe_expand_command_alias(&mut self) -> Result<(), LexError> {
    self.fill_to(self.pos)?;
    let Some(tok) = self.history.get(self.pos) else { return Ok(()) };
    let TokenKind::Word(w) = &tok.kind else { return Ok(()) };
    let Some(name) = word_literal_text(w) else { return Ok(()) };
    if self.active.contains(&name) { return Ok(()); }
    let Some(body) = self.aliases.get(&name).cloned() else { return Ok(()) };
    let name_span = tok.span;
    let body_tokens = tokenize(&body)?; // bad body → Err, propagated by callers
    self.history.remove(self.pos);
    let mut insert_at = self.pos;
    for bt in body_tokens {
        self.history.insert(insert_at, Token::new(bt.kind, name_span));
        insert_at += 1;
    }
    // Recursion guard: re-enter with `name` active so the body's own first word
    // expands if it is a *different* alias, but `name` cannot re-expand itself.
    self.active.insert(name.clone());
    self.maybe_expand_command_alias()?;
    self.active.remove(&name);
    self.alias_trailing_eligible = body.chars().last().is_some_and(|c| c.is_whitespace());
    Ok(())
}

pub fn peek_command_kind(&mut self) -> Result<Option<&TokenKind>, LexError> {
    self.maybe_expand_command_alias()?;
    self.peek_kind()
}
pub fn next_command(&mut self) -> Result<Option<Token>, LexError> {
    self.maybe_expand_command_alias()?;
    self.next()
}
```
**Verify the `active`/recursion ordering against `alias_expand.rs:216-238` exactly** (insert name BEFORE expanding the body, remove AFTER); the `alias_recursion_guard_terminates` test pins it. The `alias_trailing_eligible` flag is set here; Task 5 confirms the trailing-blank parser behavior end-to-end via the `alias*` harnesses (the unit test above covers the lexer-local case).

- [ ] **Step 5: Run — verify pass.** `cargo test -p huck-syntax alias_ bad_alias quoted_word 2>&1 | grep "test result"`; then `cargo test -p huck-syntax 2>&1 | grep "test result"`.

- [ ] **Step 6: Workspace sanity (methods still unused) + commit**

Run: `cargo test --workspace 2>&1 | grep "test result" | awk '{p+=$3;f+=$5} END{print p,f}'` (~3880, 0). `cargo build --workspace 2>&1 | grep -c warning` (→ 0; the new methods are `pub`, so no dead-code warning).
```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v239 T4: Lexer alias storage + command-position expansion (fallible; ported from Expander)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Parser calls command-position methods; REPL goes live; other callers adapt to `&mut Lexer`

Hook the two command-position sites to `peek_command_kind`/`next_command` (with `?`), change `parse` to take `&mut Lexer`, and make the **REPL** build a live `Lexer` with the alias map (read-time alias). The other `parse` callers — continuation, command-substitution bodies, function reconstruction, prompt cmdsub — are **alias-free today** (none call `expand_aliases_in_tokens`), so keep them byte-identical: keep `tokenize` then wrap the vec in `Lexer::from_tokens` (replay, no aliases) and call `parse(&mut lx)`. **Do NOT pass the alias map to `$(...)` bodies** — current huck does not expand aliases inside command substitution; doing so would diverge. The `source` loop goes live in Task 6.

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (sites 1115, 2381; `parse` signature)
- Modify: `crates/huck-engine/src/shell.rs` (REPL ≈380-401), `continuation.rs` (≈50-77), `crates/huck-syntax/src/lexer.rs` (`parse_substitution_body` ≈2876), `shell_state.rs` (≈687), `prompt.rs` (≈249)

**Interfaces:**
- Consumes: `Lexer::new_live`, `peek_command_kind`, `next_command`, `set_aliases`, `from_tokens`.
- Produces: `pub fn parse(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError>` (signature change); `pub fn new_live(input, &aliases, opts) -> Lexer` on `Lexer`.

- [ ] **Step 1: Hook the two command-position sites**

In `parse_command_inner` (`command.rs:1111-1115`):
```rust
if matches!(iter.peek_command_kind()?, Some(TokenKind::Word(_))) {
    let word_line = iter.current_line()?;
    let Some(TokenKind::Word(w)) = iter.next_command()?.map(|t| t.kind) else { unreachable!() };
```
In `parse_next_stage` (`command.rs:2378-2381`): the same change (`peek_command_kind()?` / `next_command()?.map(|t| t.kind)`). These are the ONLY two sites that use the `*_command` variants.

- [ ] **Step 2: Change `parse` signature; expose `Lexer::new_live`**

In `lexer.rs`:
```rust
pub fn new_live(input: &'a str, aliases: &std::collections::HashMap<String, String>, opts: LexerOptions) -> Lexer<'a> {
    let mut lx = Lexer::new(input, opts, true);
    lx.aliases = aliases.clone();
    lx
}
```
In `command.rs`, `parse` becomes a thin wrapper (callers now own the lexer):
```rust
pub fn parse(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    parse_cursor(iter)
}
```
Note: the `Vec<Token>` callers that don't alias-expand (continuation, comsub, function-body, prompt — Step 4) now wrap with `Lexer::from_tokens(...)`; the alias-expanding REPL (Step 3) and the source loop (Task 6) use `new_live`.

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
    Err(e) => {
        { let mut err = crate::executor::err_writer(err_sink, sink);
          e!(&mut *err, "huck: syntax error: {}", crate::parse_error_message(&e)); }
        ExecOutcome::Continue(2)
    }
}
```
(Match the EXACT existing error-message format strings in `process_line_in_sinks` — a `ParseError::Lex` formats via the Task 3 message arm, so `parse_error_message(&e)` already yields the right lex text. If the current code special-cased the lex-error prefix, preserve that wording.)

- [ ] **Step 4: continuation, comsub, function-body, prompt — adapt to `&mut Lexer` (replay, no aliases, byte-identical)**

These callers are alias-free today; keep them so. They keep calling `tokenize`/`tokenize_with_opts` (which still exists and surfaces lex errors up front exactly as now) and only swap the `parse(vec)` call for `parse(&mut Lexer::from_tokens(vec))`.

`continuation.rs:classify`: the early lex-error classification block (the `tokenize_with_opts` arms for `UnterminatedHeredoc`/unterminated quote) is **unchanged**. Only the two `command::parse(...)` calls change:
```rust
if let Err(ParseError::UnterminatedDoubleBracket) =
    command::parse(&mut lexer::Lexer::from_tokens(tokens.clone()))
{
    return Completeness::Incomplete(ContinuationReason::DoubleBracket);
}
// the trailing-operator check on `tokens.last()` is UNCHANGED (still has the vec)
...
match command::parse(&mut lexer::Lexer::from_tokens(tokens)) { /* arms unchanged */ }
```

`lexer.rs:parse_substitution_body`: keep tokenizing the body and zeroing inner lines exactly as today; only swap `command::parse(tokens)` for the `&mut Lexer` form. **No alias map.** Signature unchanged:
```rust
fn parse_substitution_body(body: &str, opts: LexerOptions) -> Result<crate::command::Sequence, LexError> {
    let mut tokens = tokenize_with_opts(body, opts).map_err(|e| LexError::Substitution(Box::new(e)))?;
    for t in &mut tokens { t.span.line = 0; }                 // unchanged: $LINENO isolation
    let mut lx = Lexer::from_tokens(tokens);
    let parsed = crate::command::parse(&mut lx).map_err(LexError::SubstitutionParseError)?;
    Ok(parsed.unwrap_or_else(empty_sequence))
}
```
(Use the EXACT `LexError` variant names the current `parse_substitution_body` uses for the tokenize-error and parse-error wraps; the two callers `scan_paren_substitution`/`scan_backtick_substitution` are unchanged.)

`shell_state.rs:687` and `prompt.rs:249`: replace `command::parse(tokens)` with `command::parse(&mut crate::lexer::Lexer::from_tokens(tokens))` (keep the preceding `tokenize(&src)` as-is). No aliases.

- [ ] **Step 5: Build, full suite, harness sweep**

Run: `cargo test --workspace 2>&1 | grep "test result" | awk '{p+=$3;f+=$5} END{print p,f}'` (~3880, 0). Then the release harness sweep (Task 3 Step 6 command); the `alias*` harnesses must be clean — the REPL is now read-time-alias.

- [ ] **Step 6: Commit**
```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/lexer.rs crates/huck-engine/src/shell.rs crates/huck-engine/src/continuation.rs crates/huck-engine/src/shell_state.rs crates/huck-engine/src/prompt.rs
git commit -m "v239 T5: parser command-position alias calls; REPL read-time alias; other callers via from_tokens

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `source`/`-c` loop live + between-unit `set_aliases`; retire `alias_generation` hack

Drive the source loop with a single live `Lexer` over the chunk, refresh its alias map between units (def-then-use), and remove the `alias_generation` re-tokenize path (the live pull picks up new aliases lazily).

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`run_sourced_contents_in_sinks` 6309-6579)

**Interfaces:**
- Consumes: `Lexer::new_live`, `set_aliases`, `parse_one_unit(&mut Lexer)`, `peek_span`, `remaining`.

- [ ] **Step 1: Replace tokenize_partial + pre-pass + from_tokens with one live lexer**

Replace the per-chunk `tokenize_partial` → `expand_aliases_in_tokens` → `Lexer::from_tokens` (≈6350-6412) with:
```rust
let opts = crate::lexer::LexerOptions { extglob, ..Default::default() };
let empty = std::collections::HashMap::new();
let aliases_now = if expand { shell.aliases.clone() } else { empty.clone() };
let mut iter = crate::lexer::Lexer::new_live(&contents[start..], &aliases_now, opts);
```
The `iter.peek_span().ok().flatten().map(|sp| sp.offset)` offset reads and `iter.remaining()` work unchanged on the live lexer; offsets are chunk-relative as before.

- [ ] **Step 2: Refresh aliases between units; drop the `alias_generation` gate**

After each unit executes, before the next `parse_one_unit`, refresh the lexer's aliases if expansion is active:
```rust
if expand {
    iter.set_aliases(shell.aliases.clone());
}
```
Remove the `alias_gen`/`shell.alias_generation` snapshot (≈6386) and the `'outer` re-tokenize restart clause (≈6507-6509) — the live lexer reflects the refreshed map for the next unit. Keep any `extglob`-change handling that is NOT alias-related; if `extglob` can change mid-source, rebuild `iter`'s `opts` accordingly (verify against the existing extglob-in-source harness).

- [ ] **Step 3: Build, full suite, harness sweep — def-then-use must work**

Add a bash-diff fragment (or Rust integration test) for cross-unit def-then-use in a sourced file:
```
alias greet='echo hi'
greet
```
must print `hi`, while same-unit
```
alias greet='echo hi'; greet
```
must treat `greet` as a command (same-unit non-expansion). Run the full suite + harness sweep; `source`/`alias` harnesses clean.

- [ ] **Step 4: Commit**
```bash
git add crates/huck-engine/src/builtins.rs
git commit -m "v239 T6: source/-c loop live + between-unit set_aliases; retire alias_generation hack

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Retire the alias pre-pass; direct incrementality + error tests; cleanup

Delete the now-unused `huck-engine` alias pre-pass and add the remaining direct tests (parser-level incrementality, bad-alias-body error, `$(...)`-not-expanded).

**Files:**
- Modify/Delete: `crates/huck-engine/src/alias_expand.rs` (delete `expand_aliases_in_tokens`/`_mapped`/`Expander` if unreferenced; grep first — keep `simple_word_text` only if still used elsewhere)
- Modify: `crates/huck-engine/src/shell_state.rs` (remove `alias_generation` field if now unreferenced; the `alias`/`unalias` builtins drop the `+= 1` bumps)
- Test: `crates/huck-syntax/src/lexer.rs`, `crates/huck-engine` integration tests
- Add: `tests/scripts/alias_readtime_diff_check.sh`

- [ ] **Step 1: Grep for remaining references, then delete the pre-pass**

Run: `rg 'expand_aliases_in_tokens|alias_generation|Expander' crates/`
Remove every now-dead path: the `alias_expand` pre-pass entry points, the `alias_generation` field (`shell_state.rs:428`) and its `+= 1` bumps (`builtins.rs:6633/6656/6665`) if nothing else reads them. Keep `simple_word_text` only if still referenced (the lexer port has its own `word_literal_text`).

- [ ] **Step 2: Direct incrementality + error tests (lexer.rs)**

```rust
#[cfg(test)]
pub fn scanned_token_count(&self) -> usize { self.history.len() }
```
```rust
#[test]
fn parser_pull_is_incremental_not_batch() {
    let input = (0..50).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("\n");
    let empty = std::collections::HashMap::new();
    let mut lx = Lexer::new_live(&input, &empty, LexerOptions::default());
    let _ = crate::command::parse_one_unit(&mut lx).unwrap();
    assert!(lx.scanned_token_count() < 10, "scanned too much: not incremental");
}

#[test]
fn bad_alias_body_surfaces_as_parse_error() {
    let mut m = std::collections::HashMap::new();
    m.insert("x".to_string(), "echo \"".to_string()); // unterminated quote in body
    let mut lx = Lexer::new_live("x", &m, LexerOptions::default());
    let r = crate::command::parse(&mut lx);
    assert!(matches!(r, Err(crate::command::ParseError::Lex(_))));
}
```

- [ ] **Step 3: `$(...)`-not-expanded + new bash-diff harness**

Create `tests/scripts/alias_readtime_diff_check.sh` modeled on the existing `alias*_diff_check.sh` harness shape, covering: command-position vs argument-position; recursion; trailing-blank; alias to a reserved word (`alias x='if'`); cross-unit def-then-use; **alias NOT expanded inside `$(...)`** (matches current huck); quoted word not expanded. Each fragment run through bash and `target/release/huck`, asserting identical output.

- [ ] **Step 4: Full suite + ALL harnesses (incl. the new one)**

Run: `cargo test --workspace 2>&1 | grep "test result" | awk '{p+=$3;f+=$5} END{print p,f}'` (≥3884, 0).
Run the release harness sweep over `tests/scripts/*_diff_check.sh` (now 153 scripts) — all clean.

- [ ] **Step 5: Commit**
```bash
git add -A
git commit -m "v239 T7: retire alias pre-pass + alias_generation; incrementality/error tests; alias_readtime harness

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **Byte-identical is the law.** After each task run the full `cargo test --workspace` and (Tasks 3/5/6/7) the release harness sweep. A red harness is a stop-and-fix.
- **The `alias*` harnesses are the alias oracle.** If one changes output, the port of the `Expander` mechanics is wrong — re-check `alias_expand.rs:216-238` (recursion `active` ordering) and `feed_word` eligibility (`alias_expand.rs:159-176`).
- **Do not `.unwrap()`/`.ok()` a pull error to dodge the Task 3 cascade** — propagate it. The single exception is the `builtins.rs` source-loop `peek_span` offset read, which is not in a `ParseError`-returning fn and uses `.ok().flatten()` deliberately (a replay/live lexer surfaces the real error on the next `parse_one_unit`).
- **`Box<LexError>` stays.** It is the stopgap for the lexer→parser recursive-type coupling; the proper fix is Phase C (see the re-arch design doc). Do not flag it for removal.
- **Incremental error ordering:** a line with both an early parse error and a later lex error now reports the parse error first (bash-aligned). If a harness pins the old batch order, treat it as an intentional change and note it in the merge.
- **`$LINENO` through aliases / `$()`**: body tokens inherit the alias-name span (Task 4); inner `$()` lines are zeroed (Task 5). The `lineno*`/`comsub*` harnesses pin this.
