# v240 — Lexer Mode Stack + mark/rewind Machinery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a dormant `modes: Vec<Mode>` stack and `mark`/`rewind`-with-cursor-reset to the `Lexer`, with `Mode::Command` running today's `scan_step` body verbatim, so behavior is byte-identical.

**Architecture:** Two tasks. Task 1 adds the `Mode` enum, the `modes` stack with push/pop, and a one-line `scan_step` dispatch (existing body moves verbatim under `scan_step_command`). Task 2 adds `CharCursor::seek`, the `Mark` struct, and `mark`/`rewind`, which use v237 token spans to locate the rewind point. Everything is new/dormant machinery reached only by unit tests; the parser is untouched.

**Tech Stack:** Rust, single file `crates/huck-syntax/src/lexer.rs`.

## Global Constraints

- **Byte-identical:** `cargo test --workspace` green AND the full `tests/scripts/*_diff_check.sh` release sweep byte-identical; 0 warnings.
- **Dormant:** no production path pushes a non-`Command` mode or calls `mark`/`rewind`; the parser (`command.rs`) is NOT touched.
- **No `TokenKind` changes** this iteration; no flag→struct refactor (the ~10 existing `Lexer` flags stay as fields).
- **All changes confined to `crates/huck-syntax/src/lexer.rs`.** No changes to `command.rs`, `errors.rs`, or any engine crate.
- New public surface is `pub(crate)` (the Stage-2 parser, same crate, will use it).
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## File Structure

- Modify only `crates/huck-syntax/src/lexer.rs`:
  - `CharCursor` (struct at line ~49): add `seek`.
  - `Lexer` (struct at line ~490): add `modes` field; init in `new` (~518) and `from_tokens` (~1230).
  - `scan_step` (~546): rename body to `scan_step_command`; add a dispatch `scan_step`.
  - Add `enum Mode`, `struct Mark`, and methods `current_mode`/`push_mode`/`pop_mode`/`mark`/`rewind`.
  - Add unit tests in the existing `#[cfg(test)]` module that hosts the `next_token_*` tests (around line ~4791).

---

### Task 1: Mode enum + mode stack + byte-identical scan_step dispatch

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (struct `Lexer` ~490, `new` ~518, `from_tokens` ~1230, `scan_step` ~546)
- Test: `crates/huck-syntax/src/lexer.rs` (`#[cfg(test)]` module near line ~4791)

**Interfaces:**
- Produces (used by Task 2 and the Stage-2 parser):
  - `pub(crate) enum Mode { Command, Subshell, CommandSub, ParamExpansion, Arith, ArrayLiteral, DoubleBracket, Regex, HeredocBody }` (derives `Debug, Clone, Copy, PartialEq, Eq`).
  - `Lexer` field `modes: Vec<Mode>` (always non-empty; `Command` is the floor).
  - `fn current_mode(&self) -> Mode`, `pub(crate) fn push_mode(&mut self, m: Mode)`, `pub(crate) fn pop_mode(&mut self) -> Mode`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` module that contains `next_token_yields_each_token_in_order` (near line ~4791):

```rust
#[test]
fn mode_stack_push_pop_current() {
    let mut lx = Lexer::new("echo hi", LexerOptions::default(), true);
    assert_eq!(lx.current_mode(), Mode::Command);
    lx.push_mode(Mode::Arith);
    assert_eq!(lx.current_mode(), Mode::Arith);
    lx.push_mode(Mode::CommandSub);
    assert_eq!(lx.current_mode(), Mode::CommandSub);
    assert_eq!(lx.pop_mode(), Mode::CommandSub);
    assert_eq!(lx.pop_mode(), Mode::Arith);
    assert_eq!(lx.current_mode(), Mode::Command);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p huck-syntax mode_stack_push_pop_current 2>&1 | tail -20`
Expected: FAIL — compile error `cannot find type Mode` / `no method named push_mode`.

- [ ] **Step 3: Implement the Mode enum, the stack, and the scan_step dispatch**

(a) Add the `Mode` enum just above `pub struct Lexer<'a>` (~line 489):

```rust
/// The lexing-rule context the lexer scans under. v240 implements only
/// `Command`; the other variants are forward declarations for later Phase C
/// iterations and are never the active mode in production yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Command,        // default: today's scan_step body (the ONLY mode implemented in v240)
    Subshell,       // ( … )
    CommandSub,     // $( … ) / `…`
    ParamExpansion, // ${ … }
    Arith,          // $(( … )) / (( … )) / $[ … ]
    ArrayLiteral,   // a=( … )
    DoubleBracket,  // [[ … ]]
    Regex,          // RHS of =~
    HeredocBody,    // <<EOF …
}
```

(b) Add the field to `struct Lexer` (after `replay: bool,` at ~line 499):

```rust
    /// Parser-controlled lexing-mode stack (Phase C). Never empty; `Command` is
    /// the floor. Dormant in v240 — only `Command` is pushed in production.
    modes: Vec<Mode>,
```

(c) Initialize it in `new` (in the `Lexer { … }` literal at ~line 519, e.g. after `replay: false,`):

```rust
            modes: vec![Mode::Command],
```

(d) Initialize it in `from_tokens` (in the `Lexer { … }` literal at ~line 1231, e.g. after `replay: true,`):

```rust
            modes: vec![Mode::Command],
```

(e) Add the stack accessors inside `impl<'a> Lexer<'a>` (place them right above the current `fn scan_step`, ~line 546):

```rust
    fn current_mode(&self) -> Mode {
        *self.modes.last().expect("mode stack is never empty (Command is the floor)")
    }

    pub(crate) fn push_mode(&mut self, m: Mode) {
        self.modes.push(m);
    }

    pub(crate) fn pop_mode(&mut self) -> Mode {
        let m = self.modes.pop().expect("pop_mode on an empty mode stack");
        debug_assert!(!self.modes.is_empty(), "Command is the floor and must never be popped");
        m
    }
```

(f) Rename the existing `fn scan_step` to `fn scan_step_command` (change ONLY its signature line at ~546 from `fn scan_step(&mut self) -> Result<Step, LexError> {` to `fn scan_step_command(&mut self) -> Result<Step, LexError> {`; leave the entire body unchanged), and add the dispatch immediately above it:

```rust
    /// Scan one step under the current mode. v240: only `Command` is implemented;
    /// any other active mode is a bug (production never pushes one yet).
    fn scan_step(&mut self) -> Result<Step, LexError> {
        match self.current_mode() {
            Mode::Command => self.scan_step_command(),
            other => unreachable!("Mode::{other:?} not implemented until its Phase C iteration"),
        }
    }
```

- [ ] **Step 4: Run the test and the suite to verify pass + byte-identical**

Run: `cargo test -p huck-syntax mode_stack_push_pop_current 2>&1 | tail -5`
Expected: PASS.

Run: `cargo test -p huck-syntax 2>&1 | grep -E "test result|warning:" | tail -5`
Expected: all pass (the existing `next_token_drain_equals_tokenize` and every other lexer test still green — proves the `scan_step` rename + dispatch is byte-identical), 0 warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v240 T1: Mode enum + dormant mode stack + scan_step dispatch

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: CharCursor::seek + Mark + mark/rewind-with-cursor-reset

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`impl CharCursor` ~58, `impl Lexer` near the stack accessors)
- Test: `crates/huck-syntax/src/lexer.rs` (`#[cfg(test)]` module near line ~4791)

**Interfaces:**
- Consumes (from Task 1): `Mode`, `self.modes`, `current_mode`.
- Consumes (existing): `CharCursor::offset()/line()/column()`; `Lexer::next_token`, `fill_to`, `history`, `pos`, `replay`, and the flag fields `brace_expand/has_token/in_assignment_value/dbracket_depth/expect_regex/opts/alias_trailing_eligible`; `Token { kind, span }`, `Span { offset, line, column }`.
- Produces:
  - `pub fn CharCursor::seek(&mut self, offset: usize, line: u32, column: u32)`.
  - `pub(crate) struct Mark` (derives `Debug, Clone`).
  - `pub(crate) fn Lexer::mark(&self) -> Mark`, `pub(crate) fn Lexer::rewind(&mut self, m: &Mark)`.

- [ ] **Step 1: Write the failing test**

Add to the same `#[cfg(test)]` module:

```rust
#[test]
fn rewind_reproduces_tokens_same_mode() {
    let mut lx = Lexer::new("echo one two; echo three", LexerOptions::default(), true);
    let m = lx.mark();
    let first: Vec<Token> = (0..4).map(|_| lx.next_token().unwrap().unwrap()).collect();
    lx.rewind(&m);
    let again: Vec<Token> = (0..4).map(|_| lx.next_token().unwrap().unwrap()).collect();
    // `Token` equality compares kind (v237's kind-only PartialEq); compare spans
    // separately to prove the cursor reset to the exact byte offsets.
    assert_eq!(first, again);
    let first_spans: Vec<Span> = first.iter().map(|t| t.span).collect();
    let again_spans: Vec<Span> = again.iter().map(|t| t.span).collect();
    assert_eq!(first_spans, again_spans);
}
```

(If `Token` does not implement `PartialEq`, compare spans only — `assert_eq!(first_spans, again_spans)` — plus assert the same count; do not add a derive to production types just for the test.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p huck-syntax rewind_reproduces_tokens_same_mode 2>&1 | tail -20`
Expected: FAIL — `no method named mark` / `no method named rewind`.

- [ ] **Step 3: Implement seek, Mark, mark, rewind**

(a) Add `seek` to `impl<'a> CharCursor<'a>` (after `slice_from`, ~line 93):

```rust
    /// Reposition the cursor to a byte offset with explicit line/column, clearing
    /// any pending 1-char peek. Used by `Lexer::rewind` to re-lex from a checkpoint.
    pub fn seek(&mut self, offset: usize, line: u32, column: u32) {
        self.pos = offset;
        self.line = line;
        self.column = column;
        self.peeked = None;
        self.peeked_len = 0;
    }
```

(b) Add the `Mark` struct just below the `Mode` enum (~line 489):

```rust
/// A checkpoint of the lexer's scanning state. `rewind` restores it and re-lexes
/// from `resume`. Taken only at a pull boundary (no word mid-accumulation), so
/// the word-accumulation buffers need not be captured.
#[derive(Debug, Clone)]
pub(crate) struct Mark {
    pos: usize,                 // self.pos (pull index) at mark time
    resume: (usize, u32, u32),  // (offset, line, column) to resume scanning from
    brace_expand: bool,
    has_token: bool,
    in_assignment_value: bool,
    dbracket_depth: u32,
    expect_regex: bool,
    opts: LexerOptions,
    alias_trailing_eligible: bool,
    modes: Vec<Mode>,
}
```

(c) Add `mark` and `rewind` inside `impl<'a> Lexer<'a>` (next to the stack accessors from Task 1):

```rust
    /// Checkpoint the scanning state for a later `rewind`. Must be called at a
    /// pull boundary (no partial word). The resume point is the span of the
    /// next-to-hand-out token when lookahead is buffered, else the live cursor.
    pub(crate) fn mark(&self) -> Mark {
        debug_assert!(
            self.current.is_empty() && self.parts.is_empty() && !self.has_token,
            "mark() must be taken at a pull boundary (no word mid-accumulation)"
        );
        let resume = if self.pos < self.history.len() {
            let s = self.history[self.pos].span;
            (s.offset, s.line, s.column)
        } else {
            (self.cursor.offset(), self.cursor.line(), self.cursor.column())
        };
        Mark {
            pos: self.pos,
            resume,
            brace_expand: self.brace_expand,
            has_token: self.has_token,
            in_assignment_value: self.in_assignment_value,
            dbracket_depth: self.dbracket_depth,
            expect_regex: self.expect_regex,
            opts: self.opts,
            alias_trailing_eligible: self.alias_trailing_eligible,
            modes: self.modes.clone(),
        }
    }

    /// Restore a `Mark`: discard buffered/produced tokens at/after it, seek the
    /// cursor back, and restore flags + mode stack. The next pull re-lexes from
    /// the checkpoint under the now-current mode. A replay (`from_tokens`) lexer
    /// never scans, so history is left intact and only `pos`/flags are reset.
    pub(crate) fn rewind(&mut self, m: &Mark) {
        debug_assert!(m.pos <= self.history.len(), "rewind target beyond history");
        if !self.replay {
            self.history.truncate(m.pos);
            self.cursor.seek(m.resume.0, m.resume.1, m.resume.2);
        }
        self.pos = m.pos;
        self.brace_expand = m.brace_expand;
        self.has_token = m.has_token;
        self.in_assignment_value = m.in_assignment_value;
        self.dbracket_depth = m.dbracket_depth;
        self.expect_regex = m.expect_regex;
        self.opts = m.opts;
        self.alias_trailing_eligible = m.alias_trailing_eligible;
        self.modes = m.modes.clone();
    }
```

NOTE on heredocs: add a one-line doc-comment above `mark` stating that `mark`/`rewind` must not span heredoc-body collection (`pending_heredocs` is intentionally not captured); that interaction is designed when heredocs enter the mode stack.

- [ ] **Step 4: Run the primary test to verify it passes**

Run: `cargo test -p huck-syntax rewind_reproduces_tokens_same_mode 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Add the remaining mark/rewind tests and verify they pass**

Add these to the same test module:

```rust
#[test]
fn rewind_across_buffered_lookahead() {
    let mut lx = Lexer::new("alpha beta gamma", LexerOptions::default(), true);
    // Buffer history[0] without consuming it (pos stays 0) so mark() resumes
    // from the buffered token's span, not the advanced cursor.
    lx.fill_to(0).unwrap();
    assert_eq!(lx.pos, 0);
    assert!(lx.history.len() >= 1);
    let m = lx.mark();
    let a = lx.next_token().unwrap().unwrap();
    let b = lx.next_token().unwrap().unwrap();
    lx.rewind(&m);
    let a2 = lx.next_token().unwrap().unwrap();
    let b2 = lx.next_token().unwrap().unwrap();
    assert_eq!(a, a2);
    assert_eq!(b, b2);
    assert_eq!((a.span, b.span), (a2.span, b2.span));
}

#[test]
fn rewind_restores_line_and_column() {
    let mut lx = Lexer::new("a\nbb\nccc", LexerOptions::default(), true);
    let _ = lx.next_token().unwrap().unwrap(); // Word "a" (line 1)
    let _ = lx.next_token().unwrap().unwrap(); // Newline (line 1)
    let m = lx.mark();                         // at start of "bb" on line 2
    let bb1 = lx.next_token().unwrap().unwrap().span;
    assert_eq!(bb1.line, 2);
    lx.rewind(&m);
    let bb2 = lx.next_token().unwrap().unwrap().span;
    assert_eq!((bb1.offset, bb1.line, bb1.column), (bb2.offset, bb2.line, bb2.column));
}

#[test]
fn rewind_restores_mode_stack() {
    let mut lx = Lexer::new("x", LexerOptions::default(), true);
    lx.push_mode(Mode::Arith);
    let m = lx.mark();
    lx.push_mode(Mode::CommandSub);
    assert_eq!(lx.current_mode(), Mode::CommandSub);
    lx.rewind(&m);
    assert_eq!(lx.current_mode(), Mode::Arith);
    assert_eq!(lx.pop_mode(), Mode::Arith);
    assert_eq!(lx.current_mode(), Mode::Command);
}

#[test]
fn rewind_restores_scalar_flags() {
    let mut lx = Lexer::new("[[ $x =~ ab*c ]] && echo y", LexerOptions::default(), true);
    let _ = lx.next_token().unwrap().unwrap(); // [[
    let _ = lx.next_token().unwrap().unwrap(); // $x
    assert_eq!(lx.dbracket_depth, 1);          // inside [[ … ]]
    let m = lx.mark();
    while lx.next_token().unwrap().is_some() {} // drain to EOF; depth returns to 0
    assert_eq!(lx.dbracket_depth, 0);
    lx.rewind(&m);
    assert_eq!(lx.dbracket_depth, 1);          // restored from the snapshot
}

#[test]
fn rewind_on_replay_lexer_does_not_truncate() {
    let toks = tokenize_with_opts("echo hi there", LexerOptions::default()).unwrap();
    let mut lx = Lexer::from_tokens(toks);
    let m = lx.mark();
    let a = lx.next_token().unwrap().unwrap();
    let _ = lx.next_token().unwrap().unwrap();
    let len_before = lx.history.len();
    lx.rewind(&m);
    assert_eq!(lx.history.len(), len_before); // replay history is NOT truncated
    let a2 = lx.next_token().unwrap().unwrap();
    assert_eq!(a, a2);
    assert_eq!(a.span, a2.span);
}
```

Run: `cargo test -p huck-syntax rewind_ 2>&1 | grep -E "test result|FAILED" | tail -5`
Expected: all `rewind_*` tests PASS.

If `rewind_restores_scalar_flags` trips the `mark()` boundary `debug_assert` or `dbracket_depth` is not `1` after pulling `[[` and `$x`, adjust the test to mark after the exact token where `dbracket_depth == 1` (pull one more token first); do NOT weaken the production code to satisfy the test.

- [ ] **Step 6: Verify the full workspace + harness sweep are byte-identical**

Run: `cargo test --workspace 2>&1 | grep -E "test result|FAILED|warning:" | tail -5`
Expected: all pass, 0 warnings.

Run (release harness sweep):
```bash
cargo build --release 2>&1 | tail -1
H="$(pwd)/target/release/huck"; n=0; f=0
for s in tests/scripts/*_diff_check.sh; do n=$((n+1)); HUCK_BIN="$H" timeout 30 bash "$s" >/dev/null 2>&1 || { echo "FAIL: $(basename $s)"; f=$((f+1)); }; done
echo "harness: $n scripts, $f failures"
```
Expected: `0 failures`.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v240 T2: CharCursor::seek + Mark + mark/rewind-with-cursor-reset

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- Line numbers above are approximate (the file is ~9,500 lines and shifts as you edit); locate by the named symbols (`pub struct Lexer`, `fn scan_step`, `fn from_tokens`, `impl<'a> CharCursor`).
- `Mode` and `Mark` are `pub(crate)`; the `unreachable!` arm is intentional fail-loud — do not make placeholder modes scan as `Command`.
- Do not touch `command.rs` or any engine crate. If you find yourself needing to, stop — that is Stage 2, out of scope.
- The byte-identical guarantee for the `scan_step` rename rests on the existing suite (especially `next_token_drain_equals_tokenize`) plus the harness sweep; there is no behavior to add in Task 1 beyond the dormant stack.
