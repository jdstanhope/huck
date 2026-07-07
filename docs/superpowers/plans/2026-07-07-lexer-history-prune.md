# Lexer History Prune + Forward-Progress Guard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound the huck lexer's `history` buffer memory (prune the consumed prefix at safe command boundaries) and turn the zero-width-opener re-emit runaway into a clean bounded error (a forward-progress guard).

**Architecture:** Two independent pieces in `crates/huck-syntax`. (1) `Lexer::maybe_prune_history()` drains the already-consumed `history[0..pos]` and resets `pos=0`; called at `parse_one_unit` and the `parse_and_or_opts` connector loop, where — by construction — no `Mark` is outstanding, so no absolute-index rebasing is needed. (2) A monotonic `consumed` counter on `CharCursor` feeds a stall detector wrapping `scan_step`: if tokens are produced without consuming input past a cap, return `LexError::NoProgress`.

**Tech Stack:** Rust; `cargo test` per-crate. Design doc: `docs/superpowers/specs/2026-07-07-lexer-history-prune-design.md`.

## Global Constraints

- **Never run `cargo test --workspace` or multi-threaded** — this box (1 core/1.9 GB) OOM-kills the session. Every test run is `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 <filter> )`.
- **Commit trailer (verbatim, every commit):** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Zero behavioral change:** parse results, error messages, and bash-diff output stay byte-identical. The full huck-syntax + huck-engine suites and the bash-diff sweep (156/2) are the no-regression gate.
- `HISTORY_PRUNE_THRESHOLD = 1024`, `SCAN_STALL_CAP = 1024` (both `usize`/`u32` consts, approved).
- All work in `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`, `crates/huck-syntax/src/errors.rs`. No new files.

---

## File map

- `crates/huck-syntax/src/lexer.rs` — `CharCursor` (`consumed` field + `next()` increment); `LexerError::NoProgress`; `SCAN_STALL_CAP` / `HISTORY_PRUNE_THRESHOLD` consts; `Lexer.stall_steps` field; `scan_step_guarded`; `maybe_prune_history`; driver-loop wiring; all unit tests for Tasks 1–3.
- `crates/huck-syntax/src/errors.rs` — `lex_error_message_impl` arm for `NoProgress`.
- `crates/huck-syntax/src/parser.rs` — two `maybe_prune_history` call sites; integration tests for Task 4.

---

## Task 1: `consumed` counter on `CharCursor`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — struct `CharCursor` (~84), `CharCursor::new` (~98), `impl Iterator for CharCursor::next` (~253)
- Test: `crates/huck-syntax/src/lexer.rs` (`mod tests`)

**Interfaces:**
- Produces: field `CharCursor.consumed: u64` — total chars yielded by `next()` (main string **and** injected alias bodies), monotonic; never decremented (rewind/seek do not touch it). Read in-module as `self.cursor.consumed`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `lexer.rs`:
```rust
#[test]
fn char_cursor_consumed_counts_yielded_chars() {
    let mut c = CharCursor::new("abc");
    assert_eq!(c.consumed, 0);
    c.next();
    c.next();
    assert_eq!(c.consumed, 2);
    c.next(); // "c"
    assert_eq!(c.consumed, 3);
    assert_eq!(c.next(), None); // EOF must NOT bump
    assert_eq!(c.consumed, 3);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 char_cursor_consumed_counts_yielded_chars )`
Expected: compile error — `no field 'consumed' on type 'CharCursor'`.

- [ ] **Step 3: Add the field, init, and increment**

In `struct CharCursor` (after the `injected` field), add:
```rust
    /// Monotonic count of chars yielded by `next()` — main string AND injected
    /// (alias-body) chars. Progress metric for the forward-progress guard; never
    /// decremented (rewind/seek reposition, they do not consume).
    consumed: u64,
```

In `CharCursor::new`, add `consumed: 0` to the literal:
```rust
    pub fn new(s: &'a str) -> Self {
        CharCursor { s, pos: 0, line: 1, column: 1, peeked: None, peeked_len: 0, injected: Vec::new(), consumed: 0 }
    }
```
(`CharCursor` is `#[derive(Clone)]` and `new` is its sole constructor — no other literal to update.)

Replace the body of `impl Iterator for CharCursor::next` with a single-result form that bumps once:
```rust
    fn next(&mut self) -> Option<char> {
        // v266: pop fully-exhausted injected frames (their bodies are drained);
        // dropping a frame releases its alias from the recursion guard, since the
        // guard derives membership from the live stack (`injected_has_alias`).
        while self.injected.last().is_some_and(Injected::exhausted) {
            self.injected.pop();
        }
        let c = if let Some(f) = self.injected.last_mut() {
            if let Some(c) = f.peeked.take() {
                f.pos += f.peeked_len;
                f.peeked_len = 0;
                Some(c)
            } else {
                // Not exhausted (the pop loop above guaranteed remaining content).
                let c = f.body[f.pos..].chars().next().expect("injected frame has content");
                f.pos += c.len_utf8();
                Some(c)
            }
        } else if let Some(c) = self.peeked.take() {
            self.pos += self.peeked_len;
            self.peeked_len = 0;
            if c == '\n' { self.line += 1; self.column = 1; } else { self.column += 1; }
            Some(c)
        } else if let Some(c) = self.s[self.pos..].chars().next() {
            self.pos += c.len_utf8();
            if c == '\n' { self.line += 1; self.column = 1; } else { self.column += 1; }
            Some(c)
        } else {
            None
        };
        if c.is_some() {
            self.consumed += 1;
        }
        c
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 char_cursor_consumed_counts_yielded_chars )`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "$(cat <<'EOF'
v267 T1: consumed counter on CharCursor

Monotonic count of chars yielded by next() (main + injected). Progress metric
for the forthcoming forward-progress guard; injected-aware so a long alias body
is real progress.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: forward-progress guard

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `enum LexError` (~3); consts; `struct Lexer` field (~890) + `Lexer::new` init (~969); new `scan_step_guarded`; `next_token` (~4319) + `fill_to` (~4355) wiring
- Modify: `crates/huck-syntax/src/errors.rs` — `lex_error_message_impl` (~78)
- Test: `crates/huck-syntax/src/lexer.rs` (`mod tests`)

**Interfaces:**
- Consumes: `CharCursor.consumed` (Task 1).
- Produces: `LexError::NoProgress`; `const SCAN_STALL_CAP: u32 = 1024`; `Lexer::scan_step_guarded(&mut self) -> Result<Step, LexError>`; `Lexer.stall_steps: u32`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `lexer.rs`:
```rust
#[test]
fn scan_stall_guard_stops_zero_width_opener_runaway() {
    // A bare `$((` at command position with NO parser to consume the ArithOpen
    // signal: scan_step re-emits it without advancing the cursor. The guard must
    // surface Err(NoProgress) within a bounded number of pulls, not loop/OOM.
    let empty = std::collections::HashMap::new();
    let mut lx = Lexer::new_live_atoms("$((", &empty, LexerOptions::default());
    let mut err = None;
    for _ in 0..(SCAN_STALL_CAP as usize + 100) {
        match lx.next() {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(e) => { err = Some(e); break; }
        }
    }
    assert!(matches!(err, Some(LexError::NoProgress)), "expected NoProgress, got {err:?}");
}

#[test]
fn scan_stall_guard_allows_normal_openers() {
    for src in ["$((1+2))", "${x}", "$(echo hi)", "`echo hi`", "$(( $(( 1 + 1 )) + 1 ))"] {
        let empty = std::collections::HashMap::new();
        let mut lx = Lexer::new_live_atoms(src, &empty, LexerOptions::default());
        assert!(crate::parser::parse_sequence(&mut lx).is_ok(), "false NoProgress on {src:?}");
    }
}

#[test]
fn scan_stall_guard_counts_injected_alias_body() {
    // An alias whose body is far longer than SCAN_STALL_CAP tokens. Each injected
    // char advances `consumed`, so the guard never fires — proving the metric is
    // injected-aware (a raw main-offset metric would false-stall here).
    let body = "a ".repeat(SCAN_STALL_CAP as usize + 500);
    let mut aliases = std::collections::HashMap::new();
    aliases.insert("x".to_string(), format!("echo {body}"));
    let mut lx = Lexer::new_live_atoms("x", &aliases, LexerOptions::default());
    assert!(crate::parser::parse_sequence(&mut lx).is_ok());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 scan_stall_guard_ )`
Expected: compile error — `no variant NoProgress`, `cannot find value SCAN_STALL_CAP`.

- [ ] **Step 3: Add the `LexError` variant + its message**

In `enum LexError` (lexer.rs ~3), add after `UnterminatedExtglob`:
```rust
    /// The scan produced tokens without consuming any input for
    /// `SCAN_STALL_CAP` steps in a row — a forward-progress safety net against a
    /// zero-width opener signal re-emitted with no parser to consume it.
    NoProgress,
```

In `lex_error_message_impl` (errors.rs), add an arm (the match is exhaustive):
```rust
        LexError::NoProgress => ": lexer made no forward progress".to_string(),
```
(The `Display` impl delegates to this; `continuation::is_unterminated_lex` must NOT list `NoProgress`, so it correctly classifies as `Completeness::Error` — leave that function unchanged.)

- [ ] **Step 4: Add the const, field, wrapper, and wiring**

Add the const near the top of `lexer.rs` (e.g. just after the `LexError` enum / `use` block, module scope):
```rust
/// Forward-progress guard: max consecutive `scan_step` calls that PRODUCE a
/// token while consuming zero input chars before `LexError::NoProgress`.
const SCAN_STALL_CAP: u32 = 1024;
```

In `struct Lexer` (~890, near `history`/`pos`), add:
```rust
    /// Consecutive `Produced` scan steps that consumed no input (see
    /// `scan_step_guarded` / `SCAN_STALL_CAP`).
    stall_steps: u32,
```
In `Lexer::new` (the struct literal ~969), add `stall_steps: 0,`.

Add the wrapper method (in the same `impl Lexer` block as `scan_step`/`next_token`):
```rust
    /// Wraps `scan_step` with a forward-progress guard: if a step PRODUCES a
    /// token without consuming any input char (`cursor.consumed` unchanged) more
    /// than `SCAN_STALL_CAP` times in a row, return `LexError::NoProgress` instead
    /// of looping forever. Catches a zero-width opener signal re-emitted with no
    /// parser to consume it (the v266-resume OOM). Any step that consumes input
    /// resets the counter, so normal parser-driven flow never trips it.
    fn scan_step_guarded(&mut self) -> Result<Step, LexError> {
        let before = self.cursor.consumed;
        let step = self.scan_step()?;
        if matches!(step, Step::Produced) {
            if self.cursor.consumed == before {
                self.stall_steps += 1;
                if self.stall_steps > SCAN_STALL_CAP {
                    return Err(LexError::NoProgress);
                }
            } else {
                self.stall_steps = 0;
            }
        }
        Ok(step)
    }
```

In `next_token` (~4325) replace `self.scan_step()?` with `self.scan_step_guarded()?`:
```rust
            match self.scan_step_guarded()? {
                Step::Eof => return Ok(None),
                Step::Produced => {}
            }
```
In `fill_to` (~4359) replace `self.scan_step()?` with `self.scan_step_guarded()?`:
```rust
            match self.scan_step_guarded()? {
                Step::Produced => {}
                Step::Eof => return Ok(()),
            }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 scan_stall_guard_ )`
Expected: PASS (3 tests).

- [ ] **Step 6: Full crate run (no regression)**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )`
Expected: all pass, 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/errors.rs
git commit -m "$(cat <<'EOF'
v267 T2: forward-progress guard on the scan loop

scan_step_guarded tracks consecutive Produced-with-zero-consume steps; past
SCAN_STALL_CAP (1024) it returns LexError::NoProgress instead of re-emitting a
zero-width opener signal forever (the v266-resume 4.8GB OOM). Injected-aware via
cursor.consumed, so long alias bodies are not false stalls; normal openers reset
the counter and pass.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `maybe_prune_history` method

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — const + method (in `impl Lexer`)
- Test: `crates/huck-syntax/src/lexer.rs` (`mod tests`)

**Interfaces:**
- Produces: `const HISTORY_PRUNE_THRESHOLD: usize = 1024` (make it `pub(crate)` — Task 4's parser tests reference it); `Lexer::maybe_prune_history(&mut self)`.
- Consumes: existing `Lexer` fields `replay`, `pos`, `history`, `pending_heredocs`, `atom_pending_heredocs`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `lexer.rs`:
```rust
#[test]
fn maybe_prune_history_drops_consumed_prefix() {
    let empty = std::collections::HashMap::new();
    let src = "a ".repeat(HISTORY_PRUNE_THRESHOLD + 50); // many simple words + blanks
    let mut lx = Lexer::new_live_atoms(&src, &empty, LexerOptions::default());
    for _ in 0..(HISTORY_PRUNE_THRESHOLD + 1) { let _ = lx.next().unwrap(); }
    assert!(lx.pos >= HISTORY_PRUNE_THRESHOLD);
    let frontier = lx.peek_kind().unwrap().cloned(); // fill + capture next token
    lx.maybe_prune_history();
    assert_eq!(lx.pos, 0, "pos reset to 0");
    assert!(lx.scanned_token_count() <= 8, "consumed prefix drained");
    assert_eq!(lx.peek_kind().unwrap().cloned(), frontier, "frontier token preserved");
}

#[test]
fn maybe_prune_history_noop_below_threshold() {
    let empty = std::collections::HashMap::new();
    let mut lx = Lexer::new_live_atoms("echo a b c", &empty, LexerOptions::default());
    let _ = lx.next().unwrap();
    let _ = lx.next().unwrap();
    let pos = lx.pos;
    let len = lx.scanned_token_count();
    lx.maybe_prune_history(); // pos < threshold → no-op
    assert_eq!(lx.pos, pos);
    assert_eq!(lx.scanned_token_count(), len);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 maybe_prune_history )`
Expected: compile error — `cannot find value HISTORY_PRUNE_THRESHOLD`, `no method maybe_prune_history`.

- [ ] **Step 3: Add the const and method**

Add the const near `SCAN_STALL_CAP` (module scope):
```rust
/// History prune threshold: prune the consumed prefix once `pos` reaches this
/// many tokens, bounding the buffer to ~one command's worth (the "at most ~1000
/// tokens" target). Not a hard cap — a single giant command still buffers O(command).
pub(crate) const HISTORY_PRUNE_THRESHOLD: usize = 1024;
```

Add the method in `impl Lexer` (near `mark`/`rewind`):
```rust
    /// Drop the consumed prefix `history[0..pos]` and reset `pos` to 0, bounding
    /// the buffer to the live (unconsumed) tail. Acts only once `pos` crosses
    /// `HISTORY_PRUNE_THRESHOLD`, to avoid churn.
    ///
    /// PRECONDITION (guaranteed at every call site): no `Mark` is outstanding — a
    /// Mark stores an absolute `pos` this would invalidate. The parser only marks
    /// inside the arith disambiguation (`parse_arith_expansion`/`parse_arith_command`),
    /// and that mark is resolved before control returns to a command/unit boundary.
    /// No-op for a replay lexer, and skipped while any heredoc body is pending
    /// (`pending_heredocs` stores history `token_idx`; a mid-collection body must
    /// not have its prefix shifted).
    pub(crate) fn maybe_prune_history(&mut self) {
        if self.replay
            || self.pos < HISTORY_PRUNE_THRESHOLD
            || !self.pending_heredocs.is_empty()
            || !self.atom_pending_heredocs.is_empty()
        {
            return;
        }
        self.history.drain(0..self.pos);
        self.pos = 0;
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 maybe_prune_history )`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "$(cat <<'EOF'
v267 T3: maybe_prune_history method

Drains history[0..pos] + resets pos=0 once pos crosses HISTORY_PRUNE_THRESHOLD
(1024). Guarded on replay + both heredoc queues empty. Not yet wired to any call
site (Task 4). pos is relative so no absolute-index rebasing is needed.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: wire the prune into the parser + integration tests

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — `parse_one_unit` (~3317), `parse_and_or_opts` connector loop (~2856)
- Test: `crates/huck-syntax/src/parser.rs` (`mod tests`)

**Interfaces:**
- Consumes: `Lexer::maybe_prune_history` (Task 3), `crate::lexer::HISTORY_PRUNE_THRESHOLD`, test-only `Lexer::scanned_token_count`.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `parser.rs` (import what's needed at the top of the test fn or module — `use crate::lexer::{Lexer, LexerOptions, HISTORY_PRUNE_THRESHOLD};` and the AST types used below):
```rust
#[test]
fn parse_one_unit_prunes_history_across_units() {
    // 5000 single-command lines on ONE lexer (the source-reader pattern). Without
    // pruning, history grows ~linearly; with the parse_one_unit prune it stays
    // bounded near HISTORY_PRUNE_THRESHOLD.
    let empty = std::collections::HashMap::new();
    let src: String = (0..5000).map(|i| format!("echo {i}\n")).collect();
    let mut lx = Lexer::new_live_atoms(&src, &empty, LexerOptions::default());
    let mut units = 0;
    while parse_one_unit(&mut lx).unwrap().is_some() {
        units += 1;
        assert!(
            lx.scanned_token_count() < HISTORY_PRUNE_THRESHOLD + 64,
            "history unbounded: {} tokens after {units} units",
            lx.scanned_token_count()
        );
    }
    assert_eq!(units, 5000);
}

#[test]
fn parse_and_or_prunes_long_semicolon_chain() {
    let empty = std::collections::HashMap::new();
    let n = HISTORY_PRUNE_THRESHOLD + 200;
    let src: String = (0..n).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("; ");
    let mut lx = Lexer::new_live_atoms(&src, &empty, LexerOptions::default());
    let seq = parse_sequence(&mut lx).unwrap().unwrap();
    assert_eq!(1 + seq.rest.len(), n, "all commands parsed");
    assert!(lx.scanned_token_count() < 2 * HISTORY_PRUNE_THRESHOLD, "pruned during parse");
}

#[test]
fn prune_does_not_break_arith_backoff_marks() {
    // A threshold-crossing chain forces a prune, then an arith-backoff construct
    // whose mark/rewind must still work relative to the pruned (pos-reset) history:
    // `$( (echo x) )` = cmdsub w/ leading subshell → parse_arith_expansion bail.
    let empty = std::collections::HashMap::new();
    let filler: String = (0..HISTORY_PRUNE_THRESHOLD + 50)
        .map(|i| format!("echo {i}")).collect::<Vec<_>>().join("; ");
    let src = format!("{filler}; echo $( (echo x) )");
    let mut lx = Lexer::new_live_atoms(&src, &empty, LexerOptions::default());
    assert!(parse_sequence(&mut lx).unwrap().is_some());
}

#[test]
fn prune_inside_nested_command_sub() {
    let empty = std::collections::HashMap::new();
    let inner: String = (0..HISTORY_PRUNE_THRESHOLD + 50)
        .map(|i| format!("echo {i}")).collect::<Vec<_>>().join("; ");
    let src = format!("x=$({inner})");
    let mut lx = Lexer::new_live_atoms(&src, &empty, LexerOptions::default());
    assert!(parse_sequence(&mut lx).unwrap().is_some());
}

#[test]
fn prune_preserves_heredoc_body_across_threshold() {
    // Heredoc redirect, then enough `;`-commands on the SAME line to cross the
    // threshold BEFORE the body, then the body. The prune must skip while the
    // heredoc is pending, so the body still attaches.
    use crate::command::{Command, SimpleCommand, RedirOp};
    use crate::lexer::WordPart;
    let empty = std::collections::HashMap::new();
    let filler: String = (0..HISTORY_PRUNE_THRESHOLD + 50)
        .map(|i| format!("; echo {i}")).collect();
    let src = format!("cat <<EOF{filler}\nBODYLINE\nEOF\n");
    let mut lx = Lexer::new_live_atoms(&src, &empty, LexerOptions::default());
    let seq = parse_one_unit(&mut lx).unwrap().unwrap();
    // Extract the first heredoc body from the first command.
    let Command::Pipeline(p) = &seq.first else { panic!("expected pipeline") };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!("expected exec") };
    let body = e.redirects.iter().find_map(|r| match &r.op {
        RedirOp::Heredoc { body, .. } => Some(body.clone()),
        _ => None,
    }).expect("heredoc redirect present");
    let text: String = body.0.iter().filter_map(|part| match part {
        WordPart::Literal { text, .. } => Some(text.clone()),
        _ => None,
    }).collect();
    assert!(text.contains("BODYLINE"), "heredoc body lost across prune: {text:?}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 prune )`
Expected: `parse_one_unit_prunes_history_across_units` FAILS its `history unbounded` assertion (the prune is not wired yet, so history grows ~linearly). The others may pass already (correctness) — the wiring must keep them passing.

- [ ] **Step 3: Wire the two call sites**

In `parse_one_unit` (parser.rs ~3317), add the prune as the first statement of the function body (before any parsing):
```rust
pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    iter.maybe_prune_history(); // bound history across units on the shared lexer
    // ...existing body...
```

In `parse_and_or_opts` (parser.rs ~2856), inside the `loop`, immediately after the leading-`Blank`-skip `while` and before the stop checks, add:
```rust
        // Bound history within a long sequence; safe here — no Mark is outstanding
        // at a command boundary (the arith disambiguation never straddles it).
        iter.maybe_prune_history();
```
Place it right after:
```rust
        while matches!(iter.peek_kind()?, Some(TokenKind::Blank)) {
            iter.next_kind()?;
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 prune )`
Expected: PASS (5 tests), including the bounded-history assertion.

- [ ] **Step 5: Full crate run (no regression)**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )`
Expected: all pass, 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "$(cat <<'EOF'
v267 T4: wire maybe_prune_history into parse_one_unit + parse_and_or_opts

parse_one_unit prunes once per top-level unit (bounds long sourced scripts / -c
on the shared lexer); parse_and_or_opts prunes per connector (bounds a long
single-line ;-chain). Both sites are mark-free command boundaries, so pruning is
safe. Integration tests cover the memory bound, ;-chain, arith-backoff marks,
nested cmdsub, and heredoc-body survival across a prune.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: full-suite + bash-diff gate

**Files:** none (verification only).

**Interfaces:** none.

- [ ] **Step 1: huck-syntax suite**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )`
Expected: all pass, 0 warnings.

- [ ] **Step 2: huck-engine suite (the deletion touched shared lexer behavior indirectly)**

Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 )`
Expected: all pass (~1740), 0 warnings.

- [ ] **Step 3: Build the binary + run the bash-diff sweep**

```bash
( ulimit -v 2500000; cargo build -p huck --jobs 1 )
export HUCK_BIN=$(pwd)/target/debug/huck
pass=0; fail=0; fails=""
for f in tests/scripts/*_diff_check.sh; do
  ( ulimit -v 1500000; timeout 60 bash "$f" ) >/dev/null 2>&1 && pass=$((pass+1)) || { fail=$((fail+1)); fails="$fails $(basename $f)"; }
done
echo "sweep: pass=$pass fail=$fail"; echo "FAILS:$fails"
```
Expected: `pass=156 fail=2`, FAILS = `cmdsub_comment_diff_check.sh funcnest_diff_check.sh` (both pre-existing). Any other failure is a regression — stop and investigate.

- [ ] **Step 4: (no commit — verification task)**

If all gates pass, the branch is ready for the iteration's final review + merge (handled outside this plan).

---

## Self-review notes (author)

- **Spec coverage:** Piece 1 method → Task 3; Piece 1 call sites → Task 4; Piece 2 counter → Task 1; Piece 2 guard → Task 2; all 8 spec tests mapped (T2 covers guard tests 6–8; T3 covers method; T4 covers prune tests 1–5); gates → Task 5.
- **Placeholder scan:** none — every code/test step carries full code.
- **Type consistency:** `consumed: u64` (T1) read in `scan_step_guarded` (T2); `HISTORY_PRUNE_THRESHOLD: usize` / `SCAN_STALL_CAP: u32` used consistently; `LexError::NoProgress` defined T2, rendered T2; `maybe_prune_history` defined T3, called T4; AST path `Command::Pipeline → commands[0] → Command::Simple(SimpleCommand::Exec) → redirects → RedirOp::Heredoc { body }` matches `command.rs`.
