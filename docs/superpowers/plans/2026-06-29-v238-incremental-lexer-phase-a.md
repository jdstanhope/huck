# v238 — Incremental Lexer Phase A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Reimplement `crates/huck-syntax/src/lexer.rs::tokenize_partial_inner` as an incremental, pull-based `Lexer` driver exposing `next_token()`, behind the UNCHANGED public `tokenize*` APIs, with byte-identical output.

**Architecture:** A `Lexer<'a>` struct owns the `CharCursor`, a `history: Vec<Token>` buffer, a `pos` cursor into it, and the former loop-local mode flags as fields. `scan_step()` is exactly one iteration of the old loop (appends 0..N tokens to `history`); `next_token()` drains `history`, calling `scan_step()` only when the buffer is dry — and never hands out a `Heredoc` token whose body backfill is still pending. `tokenize_partial_inner` becomes a loop that builds its `Vec<Token>` purely by calling `next_token()`.

**Tech Stack:** Rust, `huck-syntax` crate.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-06-29-v238-incremental-lexer-phase-a-design.md` (binding). Umbrella: `…/2026-06-29-incremental-lexer-rearch-design.md`.
- **NO TRICKS:** `next_token` scans lazily from the cursor; it must NOT pre-run the batch tokenizer and dole out from a pre-built vec. `tokenize()` is BUILT by looping `next_token()` into a vec — no other token source exists.
- **Byte-identical output.** Pure internal refactor. No public signature change to `tokenize` / `tokenize_with_opts` / `tokenize_partial` / `tokenize_no_brace`; no change to `Token`/`Span`/`LexerOptions`/`LexError`; no parser change; no `Mode` stack (that is Phase A.2).
- **Heredoc readiness:** never hand out a `Heredoc` token before its deferred body is backfilled.
- **Verification oracle:** `cargo test --workspace` (currently 3864 pass) + all 152 `tests/scripts/*_diff_check.sh` harnesses against the **release** binary (`HUCK_BIN=target/release/huck`), 0 warnings.
- Run the full workspace suite with `cargo test --workspace` (plain `cargo test` skips most crates).

---

## File Structure

- **Modify:** `crates/huck-syntax/src/lexer.rs` — only `tokenize_partial_inner` and the new `Lexer` impl. The ~30 leaf scanner fns (`scan_*`, `collect_*`) are unchanged (they already take `&mut CharCursor` + values). The four public entry points keep their bodies that route through `tokenize_partial_inner`.
- **Test:** same file's `#[cfg(test)] mod tests` — add the `next_token` tests.

No other files change.

---

## Task 1: Wrap lexer state in a `Lexer` struct (pure rename, still batch internally)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`tokenize_partial_inner` ~479–1150)

**Interfaces:**
- Produces: `struct Lexer<'a>` with fields (consumed by Tasks 2–3); `Lexer::new(input, opts, brace_expand)`; `Lexer::fill_all(&mut self) -> Result<(), LexError>` (the old loop verbatim); field `history: Vec<Token>`, `cursor: CharCursor<'a>`, `pending_heredocs: VecDeque<PendingHeredoc>`.

This task is a behavior-preserving extraction: move the loop-locals into a struct and the loop into a method, with NO control-flow change yet (still fills the whole vec in one call). This isolates the mechanical rename from the incremental split (Task 2), so a reviewer can verify each independently.

- [ ] **Step 1: Define the struct and `new`.** Above `tokenize_partial_inner`, add:

```rust
struct Lexer<'a> {
    cursor: CharCursor<'a>,
    opts: LexerOptions,
    brace_expand: bool,
    history: Vec<Token>,
    pos: usize,
    parts: Vec<WordPart>,
    current: String,
    has_token: bool,
    token_start: usize,
    token_start_line: u32,
    token_start_col: u32,
    in_assignment_value: bool,
    dbracket_depth: u32,
    expect_regex: bool,
    pending_heredocs: std::collections::VecDeque<PendingHeredoc>,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str, opts: LexerOptions, brace_expand: bool) -> Self {
        Lexer {
            cursor: CharCursor::new(input),
            opts,
            brace_expand,
            history: Vec::new(),
            pos: 0,
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
}
```

- [ ] **Step 2: Move the loop body into `fill_all`, verbatim, with mechanical renames.** Add `fn fill_all(&mut self) -> Result<(), LexError>` to the `impl`. Move the existing `let result = (|| { loop { … } })();` loop body into it as the method body (returning the loop's `Result<(), LexError>` directly — the closure is no longer needed). Apply ONLY these renames inside the moved body:
  - `tokens` → `self.history`
  - `chars` → `self.cursor`
  - `opts` → `self.opts`
  - `brace_expand` → `self.brace_expand`
  - each former local (`parts`, `current`, `has_token`, `token_start`, `token_start_line`, `token_start_col`, `in_assignment_value`, `dbracket_depth`, `expect_regex`, `pending_heredocs`) → `self.<name>`
  - The `take_fd_prefix!` macro stays a `macro_rules!` defined at the top of `fill_all`, but its body now refers to `self.history` (pop the last `Token`, reuse its `.span`, push `Token::new(TokenKind::RedirFd(fd), span)`), and `fd_prefix_of(self.history.last().map(|t| &t.kind))`.
  - Leaf scanner calls (`scan_*`, `collect_heredoc_bodies`, `emit_word_with_braces`) keep their existing argument shapes, passing `&mut self.cursor`, `self.opts`, `&mut self.history`, etc. — do NOT change those fns.
  - Borrow-checker fixes where a call needs `&mut self.history` and a value at once: bind the value to a local first (e.g. `let opts = self.opts;`).

- [ ] **Step 3: Rewrite `tokenize_partial_inner` to use the struct (still batch).**

```rust
fn tokenize_partial_inner(input: &str, opts: LexerOptions, brace_expand: bool) -> PartialTokens {
    let mut lx = Lexer::new(input, opts, brace_expand);
    let result = lx.fill_all();
    match result {
        Ok(()) => (lx.history, None),
        Err(e) => {
            let off = lx.cursor.offset();
            (lx.history, Some((e, off)))
        }
    }
}
```

- [ ] **Step 4: Build + verify byte-identical.**

Run: `cargo test -p huck-syntax` → all pass.
Run: `cargo test --workspace` → 3864 pass, 0 fail.
Run: `cargo build -p huck-syntax` → 0 warnings.
Expected: identical to pre-change (pure refactor).

- [ ] **Step 5: Commit.**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v238 T1: wrap tokenizer state in a Lexer struct (pure extraction)"
```

---

## Task 2: Split `fill_all` into `scan_step` + `next_token` (the incremental conversion)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`impl Lexer`, `tokenize_partial_inner`)

**Interfaces:**
- Consumes: the `Lexer` struct from Task 1.
- Produces: `enum Step { Produced, Eof }`; `Lexer::scan_step(&mut self) -> Result<Step, LexError>`; `Lexer::next_token(&mut self) -> Result<Option<Token>, LexError>`; `Lexer::backfill_pending_at(&self, usize) -> bool`. (Consumed by Task 3 tests and, later, Phase B.)

- [ ] **Step 1: Add the `Step` enum** (module level, near `Lexer`):

```rust
enum Step { Produced, Eof }
```

- [ ] **Step 2: Convert `fill_all`'s `loop { … }` into `scan_step` (one iteration).** Replace `fn fill_all` with `fn scan_step(&mut self) -> Result<Step, LexError>` whose body is exactly ONE iteration of the old loop (delete the `loop {` / `}` wrapper). Apply this exact control-flow mapping inside that single iteration:
  - every old `continue;` (re-loop without producing) → `return Ok(Step::Produced);`
  - every old `break;` (EOF / end of input) → `return Ok(Step::Eof);`
  - the implicit "fell off the bottom of the loop body, re-loop" path → end the method with `Ok(Step::Produced)`
  - every `?` / `return Err(..)` stays unchanged (errors still propagate)
  - NOTHING else changes — the same chars are consumed and the same tokens pushed to `self.history`.

- [ ] **Step 3: Add `next_token` and `backfill_pending_at`** to the `impl`:

```rust
fn backfill_pending_at(&self, idx: usize) -> bool {
    self.pending_heredocs.iter().any(|ph| ph.token_idx == idx)
}

fn next_token(&mut self) -> Result<Option<Token>, LexError> {
    loop {
        if self.pos < self.history.len() && !self.backfill_pending_at(self.pos) {
            let t = self.history[self.pos].clone();
            self.pos += 1;
            return Ok(Some(t));
        }
        match self.scan_step()? {
            Step::Eof => return Ok(None),
            Step::Produced => {}
        }
    }
}
```

(If `PendingHeredoc`'s field is named other than `token_idx`, use the actual field — verify in `lexer.rs`.)

- [ ] **Step 4: Rewrite `tokenize_partial_inner` to drain `next_token` — the no-tricks core.**

```rust
fn tokenize_partial_inner(input: &str, opts: LexerOptions, brace_expand: bool) -> PartialTokens {
    let mut lx = Lexer::new(input, opts, brace_expand);
    let mut out = Vec::new();
    loop {
        match lx.next_token() {
            Ok(Some(t)) => out.push(t),
            Ok(None) => return (out, None),
            Err(e) => {
                let off = lx.cursor.offset();
                return (out, Some((e, off)));
            }
        }
    }
}
```

- [ ] **Step 5: Build + verify byte-identical (this is the real gate).**

Run: `cargo test --workspace` → 3864 pass, 0 fail.
Run: `cargo build -p huck-syntax` → 0 warnings.
Run the release harness sweep:
```bash
cargo build --release -p huck-cli
export HUCK_BIN="$(pwd)/target/release/huck"
fail=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 || fail=$((fail+1)); done; echo "harness scripts with problems: $fail"
```
Expected: `0`.

If any test diverges, the `continue`→`Produced` / `break`→`Eof` mapping or the heredoc readiness check is wrong — fix before proceeding.

- [ ] **Step 6: Commit.**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v238 T2: incremental next_token; tokenize built by draining it"
```

---

## Task 3: Direct `next_token` tests (multi-token validation — required deliverable)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `Lexer::new`, `Lexer::next_token` from Task 2.

These tests drive `next_token` DIRECTLY (not via `tokenize`) and must validate repeated multi-token reads. Add a small helper to drain a fresh `Lexer`:

```rust
fn drain(input: &str) -> Vec<Token> {
    let mut lx = Lexer::new(input, LexerOptions::default(), true);
    let mut v = Vec::new();
    while let Some(t) = lx.next_token().expect("lex") { v.push(t); }
    v
}
```

- [ ] **Step 1: Multi-token sequence by hand (the core requirement).**

```rust
#[test]
fn next_token_yields_each_token_in_order() {
    let mut lx = Lexer::new("echo foo | grep bar", LexerOptions::default(), true);
    assert_eq!(lx.next_token().unwrap().unwrap().kind, TokenKind::Word(Word(vec![WordPart::Literal { text: "echo".into(), quoted: false }])));
    assert_eq!(lx.next_token().unwrap().unwrap().kind, TokenKind::Word(Word(vec![WordPart::Literal { text: "foo".into(), quoted: false }])));
    assert_eq!(lx.next_token().unwrap().unwrap().kind, TokenKind::Op(Operator::Pipe));
    assert_eq!(lx.next_token().unwrap().unwrap().kind, TokenKind::Word(Word(vec![WordPart::Literal { text: "grep".into(), quoted: false }])));
    assert_eq!(lx.next_token().unwrap().unwrap().kind, TokenKind::Word(Word(vec![WordPart::Literal { text: "bar".into(), quoted: false }])));
    assert!(lx.next_token().unwrap().is_none());
}
```
(Use the existing test helpers — e.g. `w("echo")` — if they read more cleanly; match whatever the file already uses for Word construction.)

- [ ] **Step 2: Drain-equals-`tokenize` equivalence table.**

```rust
#[test]
fn next_token_drain_equals_tokenize() {
    for src in [
        "echo hi", "a 'b c' d", "x=\"a${y}b\"", "echo ${x:-def}", "v=$(cmd arg)",
        "n=$((1 + 2))", "echo `date`", "[[ $x =~ ^a.*z$ ]]", "a{1,2,3}b",
        "cat 2>&1", "one\ntwo\nthree", "cat <<EOF\nline1\nline2\nEOF\n",
    ] {
        assert_eq!(drain(src), tokenize(src).unwrap(), "stream != batch for {src:?}");
    }
}
```

- [ ] **Step 3: N-token brace step drains across calls.**

```rust
#[test]
fn next_token_brace_expansion_drains_one_at_a_time() {
    let mut lx = Lexer::new("a{1,2,3}b", LexerOptions::default(), true);
    let a = lx.next_token().unwrap().unwrap();
    let b = lx.next_token().unwrap().unwrap();
    let c = lx.next_token().unwrap().unwrap();
    assert!(lx.next_token().unwrap().is_none());
    let txt = |t: &Token| match &t.kind { TokenKind::Word(w) => single_word_text(w), _ => None };
    assert_eq!((txt(&a), txt(&b), txt(&c)),
               (Some("a1b".into()), Some("a2b".into()), Some("a3b".into())));
}
```
(Use whatever literal-text helper the test module already has in place of `single_word_text`.)

- [ ] **Step 4: 0-token whitespace step.**

```rust
#[test]
fn next_token_skips_whitespace_runs() {
    let mut lx = Lexer::new("   echo    hi   ", LexerOptions::default(), true);
    assert!(matches!(&lx.next_token().unwrap().unwrap().kind, TokenKind::Word(_)));
    assert!(matches!(&lx.next_token().unwrap().unwrap().kind, TokenKind::Word(_)));
    assert!(lx.next_token().unwrap().is_none());
}
```

- [ ] **Step 5: Heredoc readiness — body complete when handed out.**

```rust
#[test]
fn next_token_heredoc_body_is_complete_when_emitted() {
    let mut lx = Lexer::new("cat <<EOF; echo hi\nbody1\nbody2\nEOF\n", LexerOptions::default(), true);
    let mut heredoc_body = None;
    while let Some(t) = lx.next_token().unwrap() {
        if let TokenKind::Heredoc { body, .. } = &t.kind {
            heredoc_body = Some(reconstruct_heredoc_body_text(body)); // use the file's existing body inspector
        }
    }
    assert_eq!(heredoc_body.as_deref(), Some("body1\nbody2\n"));
}
```
(Use the test module's existing heredoc-body inspection helper; assert the body is the full two lines, proving the stall rule — an empty body would mean a token was handed out early.)

- [ ] **Step 6: Partial + error parity with `tokenize_partial`.**

```rust
#[test]
fn next_token_partial_error_matches_tokenize_partial() {
    let src = "echo ok \"unterminated";
    let (batch_tokens, batch_err) = tokenize_partial(src, LexerOptions::default());
    let mut lx = Lexer::new(src, LexerOptions::default(), true);
    let mut stream = Vec::new();
    let mut stream_err = None;
    loop {
        match lx.next_token() {
            Ok(Some(t)) => stream.push(t),
            Ok(None) => break,
            Err(e) => { stream_err = Some((e, lx.cursor.offset())); break; }
        }
    }
    assert_eq!(stream, batch_tokens);
    assert_eq!(stream_err.map(|(_, o)| o), batch_err.map(|(_, o)| o));
}
```

- [ ] **Step 7: Run the new tests + full suite.**

Run: `cargo test -p huck-syntax next_token` → all new tests pass.
Run: `cargo test --workspace` → 3870+ pass (3864 + new), 0 fail.

- [ ] **Step 8: Commit.**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v238 T3: direct next_token tests (multi-token sequence, drain==tokenize, heredoc, partial)"
```

---

## Self-review checklist (before final review)

- `tokenize_partial_inner` contains a `loop { … next_token() … }` and NO other token source. ✓ no-tricks
- `next_token` advances the cursor lazily (via `scan_step`), never pre-runs batch tokenize. ✓
- Heredoc readiness check present; no empty-body Heredoc test passes. ✓
- All four public entry points unchanged in signature. ✓
- `cargo test --workspace` + 152 harnesses (release) green; 0 warnings. ✓
- No `Mode` enum/stack, no parser change. ✓ (deferred to A.2 / B)
