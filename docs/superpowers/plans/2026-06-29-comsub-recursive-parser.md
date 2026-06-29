# v236: `$()` recursive-parser command substitution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace huck's heuristic byte-scanner for `$( … )` command-substitution boundaries with a true recursive parse — the parser's grammar finds the closing `)`.

**Architecture:** At `$(`, tolerantly tokenize the remaining input (`tokenize_partial`), hand the tokens to a new `command::parse_comsub` that parses a command list and stops at the command-level `RParen`, then reposition the shared `CharCursor` to just past that `)` via byte-offset remap. Heredoc bodies that span the boundary are collected by the existing pending-heredoc machinery for free; an unterminated in-comsub heredoc no longer aborts.

**Tech Stack:** Rust, crate `huck-syntax` (`lexer.rs`, `command.rs`); bash-diff harness under `tests/scripts/`.

## Global Constraints

- **Crate/files:** all code changes are in `crates/huck-syntax/src/lexer.rs` and `crates/huck-syntax/src/command.rs`. New harness in `tests/scripts/`.
- **Tests:** run `cargo test --workspace` (≈3862 tests; plain `cargo test` skips most crates). Build the release binary for the diff harness with `cargo build --release --bin huck`.
- **Branch:** implement on `v236-comsub` (cut from `main`). Do NOT push to `main` without confirmation.
- **Commit trailer (verbatim, every commit):**
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Behavior discipline:** every case that currently passes must keep passing (characterization net). Only previously-failing comsub constructs may change. The ONE intentional behavior change: unterminated-heredoc-in-comsub stops aborting.
- **Out of scope (do NOT touch):** the four non-primary `scan_cmdsub_body` callers (lexer.rs:2241 legacy `$[ ]`, 2629 & 3901 `consume_paren_cmdsub_verbatim`, 3400 subscript); backticks (`scan_backtick_substitution`); the error-message *prefix* format (error-prologue). `scan_cmdsub_body` itself STAYS (still used by those 4 callers).

## Key facts (verified against the tree)

- `scan_paren_substitution` (lexer.rs:2790-2796) is the ONLY primary `$()`→`Sequence` site. The `$((`→try-arith-else-rewind logic in `scan_dollar_expansion` (lexer.rs:1810-1834) sits ABOVE it and is unchanged.
- `tokenize_partial(input, opts) -> (Vec<Token>, Vec<usize> /*offsets*/, Vec<u32> /*lines*/, Option<(LexError, usize)>)` (lexer.rs:401-onwards). On a lex error it returns the tokens produced BEFORE the error plus `Some((err, off))`. `offsets.len() == tokens.len() + 1`; `offsets[i]` is the START byte of token `i` (raw-anchored).
- `parse_subshell_sequence(iter: &mut TokenCursor) -> Result<Sequence, ParseError>` (command.rs:1841) parses a command list, **consuming** the terminating `)`. `Err(ParseError::UnterminatedSubshell)` if the stream ends first.
- `TokenCursor` (command.rs:772) has private `pos`, `peek()`, `next()`, no public position getter yet.
- `CharCursor<'a>` (lexer.rs:49) fields `s: &'a str, pos, line, peeked, peeked_len`; methods `offset()`, `line()`, `slice_from()`. `Operator::RParen` is the single byte `)`.
- `LexError::{UnterminatedSubstitution, UnterminatedHeredoc, SubstitutionParseError(ParseError)}` (lexer.rs:3-onwards). `empty_sequence()` helper lives in lexer.rs (near 2825).

---

### Task 1: `command::parse_comsub` + `TokenCursor::pos()`

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (add `TokenCursor::pos`, add `parse_comsub`; tests in the existing `#[cfg(test)] mod`)

**Interfaces:**
- Produces: `pub fn parse_comsub(tokens: Vec<Token>, lines: Vec<u32>) -> Result<(Option<Sequence>, usize), ParseError>` — parses a command-substitution body; `Ok((Some(seq), n))` for a non-empty body, `Ok((None, n))` for empty `$()`, where `n` = tokens consumed through and including the closing `)`. `Err(ParseError::UnterminatedSubshell)` when no command-level `)` is found.
- Produces: `pub fn TokenCursor::pos(&self) -> usize`.
- Consumes: existing `parse_subshell_sequence`, `skip_newlines`, `TokenCursor`.

- [ ] **Step 1: Write failing tests** (in command.rs test module). Use the sibling lexer to build tokens:

```rust
#[test]
fn parse_comsub_simple_stops_at_close() {
    let (toks, _o, lines) = crate::lexer::tokenize_with_offsets("echo hi ) trailing", Default::default()).unwrap();
    let (seq, n) = parse_comsub(toks, lines).unwrap();
    assert!(seq.is_some());
    // consumed `echo hi )` = 4 tokens (echo, hi, ), ... ) — assert the close was consumed:
    // token at index n-1 was the RParen; trailing tokens remain unconsumed.
    assert!(n >= 3);
}

#[test]
fn parse_comsub_empty_is_none() {
    let (toks, _o, lines) = crate::lexer::tokenize_with_offsets(")", Default::default()).unwrap();
    let (seq, n) = parse_comsub(toks, lines).unwrap();
    assert!(seq.is_none());
    assert_eq!(n, 1);
}

#[test]
fn parse_comsub_case_pattern_paren_not_close() {
    // The `)` after `a` is a case-pattern terminator; only the final `)` closes.
    let (toks, _o, lines) = crate::lexer::tokenize_with_offsets("case x in a) echo a;; esac )", Default::default()).unwrap();
    let (seq, _n) = parse_comsub(toks, lines).unwrap();
    assert!(seq.is_some()); // parses end-to-end, no premature close
}

#[test]
fn parse_comsub_unterminated_errs() {
    let (toks, _o, lines) = crate::lexer::tokenize_with_offsets("echo hi", Default::default()).unwrap();
    assert!(matches!(parse_comsub(toks, lines), Err(ParseError::UnterminatedSubshell)));
}
```

- [ ] **Step 2: Run, verify they fail** (`cargo test -p huck-syntax --lib parse_comsub`) — `parse_comsub`/`pos` not defined.

- [ ] **Step 3: Implement.** Add to `impl TokenCursor`:

```rust
/// Index of the next token to be produced (number of tokens consumed so far).
pub fn pos(&self) -> usize { self.pos }
```

Add the free function near `parse_subshell_sequence`:

```rust
/// Parses a command-substitution body: a command list terminated by the
/// command-level `)`. Reuses `parse_subshell_sequence` for boundary-finding,
/// but — unlike a subshell — an empty body `$()` is VALID and yields `None`.
/// Returns the parsed body and the number of tokens consumed THROUGH the
/// closing `)`, so the lexer can map that back to a byte offset.
pub fn parse_comsub(
    tokens: Vec<Token>,
    lines: Vec<u32>,
) -> Result<(Option<Sequence>, usize), ParseError> {
    let mut iter = TokenCursor::new(tokens, lines);
    skip_newlines(&mut iter);
    // Empty comsub: `$()` / `$( )` — the body is just the close.
    if matches!(iter.peek(), Some(Token::Op(Operator::RParen))) {
        iter.next();
        return Ok((None, iter.pos()));
    }
    // No tokens before `)` → unterminated.
    if iter.peek().is_none() {
        return Err(ParseError::UnterminatedSubshell);
    }
    let seq = parse_subshell_sequence(&mut iter)?;
    Ok((Some(seq), iter.pos()))
}
```

- [ ] **Step 4: Run tests, verify they pass.** (`cargo test -p huck-syntax --lib parse_comsub`)

- [ ] **Step 5: Commit** (`feat(parser): add parse_comsub for recursive command-substitution boundary`).

---

### Task 2: `CharCursor::seek`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`impl CharCursor`; tests in the lexer test module)

**Interfaces:**
- Produces: `pub fn CharCursor::seek(&mut self, pos: usize)` — reposition to byte offset `pos`, clearing the peek buffer and recomputing the 1-based line.
- Produces: `pub fn CharCursor::source(&self) -> &'a str` — the full backing source (lifetime-independent of `&self`, so the caller can hold the slice across a later `&mut self` call).

- [ ] **Step 1: Write failing tests:**

```rust
#[test]
fn char_cursor_seek_repositions_and_counts_lines() {
    let s = "ab\ncd\nef";
    let mut c = CharCursor::new(s);
    c.seek(6); // just past the second '\n' (bytes: a b \n c d \n | e f)
    assert_eq!(c.offset(), 6);
    assert_eq!(c.line(), 3);          // two newlines skipped → line 3
    assert_eq!(c.next(), Some('e'));
}

#[test]
fn char_cursor_seek_clears_peek() {
    let mut c = CharCursor::new("xyz");
    assert_eq!(c.peek(), Some(&'x'));
    c.seek(2);
    assert_eq!(c.peek(), Some(&'z')); // stale peek discarded
}

#[test]
fn char_cursor_source_returns_full_input() {
    let c = CharCursor::new("hello");
    assert_eq!(c.source(), "hello");
}
```

- [ ] **Step 2: Run, verify they fail.** (`cargo test -p huck-syntax --lib char_cursor_seek`)

- [ ] **Step 3: Implement** in `impl<'a> CharCursor<'a>`:

```rust
/// The full backing source. Lifetime is `'a` (not tied to `&self`), so a
/// caller may keep the returned slice while later taking `&mut self` (e.g.
/// command substitution: borrow the remainder, then `seek` past the `)`).
pub fn source(&self) -> &'a str { self.s }

/// Reposition the cursor to byte offset `pos` (must be a char boundary).
/// Clears any peeked char and recomputes the 1-based line. Used by command
/// substitution to resume just after the `)` the recursive parse located.
pub fn seek(&mut self, pos: usize) {
    debug_assert!(pos <= self.s.len() && self.s.is_char_boundary(pos));
    self.peeked = None;
    self.peeked_len = 0;
    self.pos = pos;
    self.line = 1 + self.s[..pos].bytes().filter(|&b| b == b'\n').count() as u32;
}
```

(Line is recomputed from the start: O(pos), simple and robust. Acceptable at parse time; see Risks in the spec.)

- [ ] **Step 4: Run tests, verify they pass.**

- [ ] **Step 5: Commit** (`feat(lexer): add CharCursor::seek and source accessors`).

---

### Task 3: Rewire `scan_paren_substitution` to the recursive parse

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`scan_paren_substitution`; integration tests in the lexer test module)

**Interfaces:**
- Consumes: `parse_comsub` (Task 1), `CharCursor::{seek,source,offset}` (Task 2), `tokenize_partial`, `empty_sequence`, `LexError`, `crate::command::ParseError`.
- `scan_paren_substitution`'s signature is unchanged: `fn(&mut CharCursor, LexerOptions) -> Result<Sequence, LexError>`.

- [ ] **Step 1: Write failing/repurposed integration tests** (lexer test module). These describe behavior the OLD heuristic got wrong:

```rust
#[test]
fn comsub_case_pattern_paren_is_not_close() {
    // Previously mis-scanned by scan_cmdsub_body; must now tokenize fully.
    let toks = tokenize("echo $(case x in a) echo hit;; *) echo no;; esac)").unwrap();
    // The whole thing is ONE word: literal `echo ` + CommandSub. No stray RParen token.
    assert!(!toks.iter().any(|t| matches!(t, Token::Op(Operator::RParen))));
}

#[test]
fn comsub_empty_case_parses() {
    let toks = tokenize("echo $(case x in esac)").unwrap();
    assert!(!toks.iter().any(|t| matches!(t, Token::Op(Operator::RParen))));
}

#[test]
fn comsub_in_dquotes_resumes_after_close() {
    // `"pre $(echo hi) post"` — after the comsub, ` post"` must lex as string content.
    let toks = tokenize("\"pre $(echo hi) post\"").unwrap();
    // Exactly one word token (the double-quoted string), no leaked tokens.
    assert_eq!(toks.len(), 1);
}

#[test]
fn comsub_adjacent_suffix_stays_one_word() {
    // `$(echo x)y` resumes EXACTLY after `)`, keeping `y` adjacent (one word).
    let toks = tokenize("$(echo x)y").unwrap();
    assert_eq!(toks.len(), 1);
}

#[test]
fn comsub_nested_parses() {
    let toks = tokenize("echo $(echo $(echo deep))").unwrap();
    assert!(!toks.iter().any(|t| matches!(t, Token::Op(Operator::RParen))));
}

#[test]
fn comsub_unterminated_errs() {
    assert_eq!(tokenize("$(echo hi").unwrap_err(), LexError::UnterminatedSubstitution);
}
```

- [ ] **Step 2: Run, verify the new ones fail** (the case-pattern/empty-case ones fail under the old heuristic; `comsub_adjacent_suffix_stays_one_word` guards the offset math). `cargo test -p huck-syntax --lib comsub_`

- [ ] **Step 3: Implement** — replace the body of `scan_paren_substitution`:

```rust
fn scan_paren_substitution(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<crate::command::Sequence, LexError> {
    let body_start = chars.offset();
    let rest = &chars.source()[body_start..];
    // Tolerant: a trailing unterminated construct that belongs to the OUTER
    // context (e.g. the closing `"` of `"… $(x) …"`) just stops tokenization;
    // we recover the tokens produced before it.
    let (toks, offs, lines, tail_err) = tokenize_partial(rest, opts);
    let (seq_opt, n_consumed) = match crate::command::parse_comsub(toks, lines) {
        Ok(v) => v,
        Err(crate::command::ParseError::UnterminatedSubshell) => {
            // No command-level `)` was found. If tokenization was cut short by
            // a lex error INSIDE the body (e.g. an unterminated quote), surface
            // that; otherwise the substitution itself is unterminated.
            return match tail_err {
                Some((e, _)) => Err(e),
                None => Err(LexError::UnterminatedSubstitution),
            };
        }
        Err(e) => return Err(LexError::SubstitutionParseError(e)),
    };
    // A `)` was found at token index `n_consumed - 1`. Resume EXACTLY past it:
    // the close is the single byte `)`, so its end = its start + 1. (Using
    // offs[n_consumed] would skip any whitespace before the next token and
    // wrongly glue an adjacent suffix — see comsub_adjacent_suffix test.)
    let close_start = offs[n_consumed - 1];
    chars.seek(body_start + close_start + 1);
    // `tail_err` here (if any) is past the close — the outer lexer re-encounters
    // and reports it when it resumes. Ignore it. (Heredoc-EOF handled in Task 4.)
    let _ = tail_err;
    Ok(seq_opt.unwrap_or_else(empty_sequence))
}
```

- [ ] **Step 4: Run the new tests + the full lexer suite** (`cargo test -p huck-syntax`). The characterization net = the entire existing lexer/parser test suite must stay green; investigate ANY pre-existing comsub test that flips.

- [ ] **Step 5: Commit** (`feat(lexer): recursive-parser command substitution boundary`).

---

### Task 4: Heredoc-in-comsub behavior

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (tests only; plus a one-line scope check in `scan_paren_substitution` if needed)

**Interfaces:** none new. This task verifies the Task-3 logic already yields bash-parity for heredocs, and pins the unterminated-heredoc refinement.

Behavior notes:
- **Cross-boundary heredoc works for free:** `tokenize_partial(rest, …)` tokenizes the whole remainder, so the existing pending-heredoc queue collects the body from the later lines and the `)` after the terminator line is found by `parse_comsub`; `seek` jumps the cursor past the entire span.
- **Unterminated heredoc in comsub** (`$(cat <<EOF)` with no terminator before EOF): `tokenize_partial` returns `tail_err = Some((LexError::UnterminatedHeredoc, _))` with the `)` already among the tokens, so `parse_comsub` finds the close and Task-3's "ignore tail_err on success" path runs — the heredoc body stays empty and the substitution does NOT abort. **Refinement vs spec:** v236 produces the correct OUTPUT (empty body, no abort) but emits NO warning text. Matching bash's stderr warning verbatim is the separate error-prologue concern; the diff harness (Task 5) compares STDOUT for these fragments. Record the missing warning as a deferred `[low]` divergence at merge.

- [ ] **Step 1: Write tests:**

```rust
#[test]
fn comsub_with_heredoc_collects_body() {
    // The heredoc body and its terminator sit on lines AFTER `$(`.
    let toks = tokenize("x=$(cat <<EOF\nhello\nEOF\n)\necho done").unwrap();
    // Tokenizes end-to-end: an assignment word, a newline, `echo`, `done`.
    assert!(toks.iter().any(|t| matches!(t, Token::Newline)));
}

#[test]
fn comsub_unterminated_heredoc_does_not_abort() {
    // No `EOF` terminator before end-of-input: must NOT error; body empty.
    let toks = tokenize("x=$(cat <<EOF)").unwrap();
    assert_eq!(toks.len(), 1); // one assignment word, no error
}
```

- [ ] **Step 2: Run, verify.** `comsub_unterminated_heredoc_does_not_abort` previously errored with `UnterminatedHeredoc`; it now passes from Task 3's logic. If it still errors, the `tail_err`-ignore path needs the fix; otherwise no code change.

- [ ] **Step 3: Implement** — only if Step 2 shows a gap (it should pass as-is). No new code expected; this task is primarily the behavioral lock-in.

- [ ] **Step 4: Run full `cargo test -p huck-syntax`.**

- [ ] **Step 5: Commit** (`test(lexer): heredoc-across-comsub-boundary parity`).

---

### Task 5: `comsub_diff_check.sh` bash-vs-huck harness

**Files:**
- Create: `tests/scripts/comsub_diff_check.sh` (model on `tests/scripts/arith_error_diff_check.sh`)

**Interfaces:** standalone shell harness; compares STDOUT + exit status of each fragment run under `bash` and `huck`.

- [ ] **Step 1: Write the harness.** Reuse the arith_error_diff_check.sh preamble (locate `$HUCK_BIN` at `target/release/huck`, SKIP if bash absent, `PASS`/`FAIL` counters). For each fragment, write it to `./frag.sh`, run under both shells from the temp dir, and assert byte-identical STDOUT and identical exit status. Fragments:

```sh
'echo $(case x in a) echo hit;; *) echo other;; esac)'
'echo $(case x in esac)X'
'echo $(echo $(echo deep))'
'A=$(case $x in (b) echo two;; esac); echo "[$A]"'
'echo "pre $(echo mid) post"'
'echo $(echo a)$(echo b)'
'printf "%s\n" $(echo one; echo two)'
'echo $(cat <<EOF
line1
line2
EOF
)'
'echo $(( 1 + 2 ))'          # still arithmetic, not comsub
'x=$(cat <<EOF); echo after=$x'   # unterminated heredoc in comsub: no abort, after= empty
```

For the unterminated-heredoc fragment, compare STDOUT only (bash's stderr warning prefix differs — error-prologue, out of scope).

- [ ] **Step 2: Build release + run** (`cargo build --release --bin huck && bash tests/scripts/comsub_diff_check.sh`). Expected: all fragments PASS (byte-identical stdout + status). Any FAIL is a real divergence — fix in lexer/parser, not by weakening the fragment.

- [ ] **Step 3: Commit** (`test: comsub_diff_check.sh bash-vs-huck harness`).

---

### Task 6: Integration verification

**Files:** none (verification + notes for the merge step).

- [ ] **Step 1: Full workspace suite** — `cargo test --workspace`; must be green (≈3862+).
- [ ] **Step 2: Diff harness** — `bash tests/scripts/comsub_diff_check.sh` all PASS.
- [ ] **Step 3: Bash-suite re-run** (record status, do NOT commit GPL'd diffs):
  `BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_CATEGORY=comsub bash tests/bash-test-suite/runner.sh` — repeat for `comsub-eof`, `comsub-posix`, `more-exp`, `new-exp`. Note any opportunistic flips.
- [ ] **Step 4: Hand off to the iteration's final-review + merge step** (whole-branch review, then merge `--no-ff`, update `docs/bash-divergences.md` with the deferred follow-ons — verbatim-skip callers still heuristic; unterminated-heredoc-in-comsub warning text missing — and record the iteration in memory). These are owned by the controller, not a TDD task.

---

## Self-review

- **Spec coverage:** recursive boundary (Tasks 1+3), heredoc-cross-boundary + unterminated refinement (Task 4), offset/line correctness (Task 2 + the adjacent-suffix test in Task 3), `$((` arith untouched (Task 5 fragment), backticks untouched (not modified), diff harness + characterization net + bash re-run (Tasks 5-6). Covered.
- **Type consistency:** `parse_comsub` returns `(Option<Sequence>, usize)`; the lexer destructures it and `unwrap_or_else(empty_sequence)` — consistent across Tasks 1 and 3. `offs[n_consumed - 1] + 1` is the close-token end (RParen = 1 byte) — consistent with the "resume exactly past `)`" requirement.
- **Placeholder scan:** no TBDs; every code step shows the code.
- **Risk pin:** the offset math (the spec's named top risk) is guarded by `comsub_adjacent_suffix_stays_one_word` and the `CharCursor::seek` line-count tests.
