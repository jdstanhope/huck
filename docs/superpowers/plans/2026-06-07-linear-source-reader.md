# Linear-time script source reader (M-99) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck read scripts (`source`/`.`, `huck SCRIPT`, `huck -c`, `--rcfile`) in O(n) by replacing the per-line `classify` loop in `run_sourced_contents` with a tokenize-once / parse-and-execute-one-command-at-a-time reader.

**Architecture:** Add a position-tracking char cursor to the lexer so it can emit a byte-offset sidecar (`tokenize_with_offsets`). Add an opt-in "stop at a top-level newline" mode to the parser (`parse_sequence_opts` + `parse_one_unit`) so the reader can pull one command at a time. Rewrite `run_sourced_contents` to tokenize the whole script once, parse+execute unit by unit (re-lexing the remainder only when a command flips `shopt extglob`), reconstructing line numbers / `set -v` echo / source spans from the offsets. `classify` and the interactive REPL reader are untouched.

**Tech Stack:** Rust. `src/lexer.rs`, `src/command.rs`, `src/builtins.rs`. Tests: `cargo test --bin huck` (unit), `cargo test --test <name>` (integration), `bash tests/scripts/<name>_diff_check.sh` (bash-diff harness).

**Spec:** `docs/superpowers/specs/2026-06-07-linear-source-reader-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## File Structure

- `src/lexer.rs` — Task 1 adds `CharCursor` (drop-in for `Peekable<Chars>`); Task 2 adds `tokenize_with_offsets` + offset emission; `tokenize_with_opts` delegates.
- `src/command.rs` — Task 3 adds `parse_sequence_opts` + `parse_sequence` wrapper + `parse_one_unit`.
- `src/builtins.rs` — Task 4 rewrites `run_sourced_contents`.
- `tests/linear_source_reader_integration.rs` — NEW (Task 4): behavior + timing.
- `tests/scripts/source_reader_diff_check.sh` — NEW (Task 5): 29th bash-diff harness.
- `docs/bash-divergences.md`, `README.md` — Task 6.

---

## Task 1: `CharCursor` — position-tracking drop-in for `Peekable<Chars>`

Pure refactor. The lexer currently lexes over `std::iter::Peekable<std::str::Chars<'_>>`, which hides its byte position. Replace it with a `CharCursor` that behaves identically but also exposes the current byte offset. **No behavior change** — every existing lexer test must still pass.

The lexer uses exactly these methods on the iterator: `next()`, `peek()` (returns `Option<&char>`), `by_ref()`, `clone()`. `CharCursor` provides all four.

**Files:**
- Modify: `src/lexer.rs` (add `CharCursor`; change the 27 `&mut std::iter::Peekable<std::str::Chars<'_>>` signatures to `&mut CharCursor<'_>`; change the initial `let mut chars = input.chars().peekable();` in `tokenize_with_opts`).

- [ ] **Step 1: Write `CharCursor`**

Add near the top of `src/lexer.rs` (after the imports / `LexError` enum):

```rust
/// A char cursor over a `&str` that also tracks the byte offset of the next
/// char to be produced. Drop-in for the `Peekable<Chars>` the lexer used:
/// implements `Iterator<Item = char>`, a `peek()` returning `Option<&char>`,
/// `Clone`, and (via `Iterator`) `by_ref()`. `offset()` is the byte position
/// of the char that the next `next()`/`peek()` will yield (or `s.len()` at end).
#[derive(Clone)]
pub struct CharCursor<'a> {
    s: &'a str,
    pos: usize,
    peeked: Option<char>,
    peeked_len: usize,
}

impl<'a> CharCursor<'a> {
    pub fn new(s: &'a str) -> Self {
        CharCursor { s, pos: 0, peeked: None, peeked_len: 0 }
    }

    /// Peek the next char without consuming it.
    pub fn peek(&mut self) -> Option<&char> {
        if self.peeked.is_none() {
            if let Some(c) = self.s[self.pos..].chars().next() {
                self.peeked = Some(c);
                self.peeked_len = c.len_utf8();
            }
        }
        self.peeked.as_ref()
    }

    /// Byte offset of the next char to be produced (start of the next token
    /// when the cursor sits on a token boundary). Equals `s.len()` at EOF.
    pub fn offset(&self) -> usize {
        self.pos
    }
}

impl Iterator for CharCursor<'_> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        if let Some(c) = self.peeked.take() {
            self.pos += self.peeked_len;
            self.peeked_len = 0;
            Some(c)
        } else if let Some(c) = self.s[self.pos..].chars().next() {
            self.pos += c.len_utf8();
            Some(c)
        } else {
            None
        }
    }
}
```

Note: `peek()` does **not** advance `pos`; `next()` does. So `offset()` is always the start of the not-yet-consumed char — exactly the token-start byte when called at a boundary.

- [ ] **Step 2: Swap the iterator type everywhere in `src/lexer.rs`**

Replace every occurrence of the parameter type
`&mut std::iter::Peekable<std::str::Chars<'_>>` with `&mut CharCursor<'_>`
(27 sites, all the helper signatures). And in `tokenize_with_opts`, change:

```rust
let mut chars = input.chars().peekable();
```
to
```rust
let mut chars = CharCursor::new(input);
```

Do not change any call site bodies: `chars.next()`, `chars.peek()`, `chars.by_ref()`, `chars.clone()` all work unchanged on `CharCursor`.

Run a quick check that no `Peekable<...Chars` signatures remain:
`grep -n 'Peekable<std::str::Chars' src/lexer.rs` → expect no matches.

- [ ] **Step 3: Build + run the full lexer test suite**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean (fix any missed signature).

Run: `cargo test --bin huck 2>&1 | tail -15`
Expected: **all** unit tests pass (the lexer has a large test module; this is the regression gate proving `CharCursor` is behavior-identical).

- [ ] **Step 4: Add a `CharCursor` offset unit test**

Add to the `#[cfg(test)] mod tests` in `src/lexer.rs`:

```rust
#[test]
fn char_cursor_tracks_byte_offset() {
    let mut c = CharCursor::new("ab\nc");
    assert_eq!(c.offset(), 0);
    assert_eq!(c.peek(), Some(&'a'));
    assert_eq!(c.offset(), 0); // peek does not advance
    assert_eq!(c.next(), Some('a'));
    assert_eq!(c.offset(), 1);
    assert_eq!(c.next(), Some('b'));
    assert_eq!(c.next(), Some('\n'));
    assert_eq!(c.offset(), 3);
    assert_eq!(c.next(), Some('c'));
    assert_eq!(c.offset(), 4);
    assert_eq!(c.next(), None);
    assert_eq!(c.offset(), 4);
}

#[test]
fn char_cursor_offset_with_multibyte() {
    // 'é' is 2 bytes in UTF-8.
    let mut c = CharCursor::new("é!");
    assert_eq!(c.offset(), 0);
    assert_eq!(c.next(), Some('é'));
    assert_eq!(c.offset(), 2);
    assert_eq!(c.next(), Some('!'));
    assert_eq!(c.offset(), 3);
}
```

Run: `cargo test --bin huck char_cursor 2>&1 | tail -8`
Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "refactor(lexer): position-tracking CharCursor replaces Peekable<Chars>

Drop-in for the lexer's char iterator (next/peek/by_ref/clone) that also
exposes the byte offset of the next char. Pure refactor; all lexer tests
unchanged. Enables the token offset sidecar (next task).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Token byte-offset sidecar (`tokenize_with_offsets`)

Emit, alongside the tokens, a parallel `Vec<usize>` of each token's start byte offset (plus a trailing sentinel = `input.len()`). `tokenize_with_opts` becomes a thin wrapper that drops the offsets, so there is one tokenizer and its existing output is byte-identical.

**Files:**
- Modify: `src/lexer.rs` (rename the body of `tokenize_with_opts` to a core that also builds `offsets`; add `tokenize_with_offsets`; keep `tokenize_with_opts`/`tokenize` as wrappers).

- [ ] **Step 1: Write failing offset tests**

Add to `src/lexer.rs` tests:

```rust
#[test]
fn offsets_align_with_token_starts() {
    // "echo hi\nls" -> Word(echo)@0 Word(hi)@5 Newline@7 Word(ls)@8, sentinel@10
    let (toks, offs) = tokenize_with_offsets("echo hi\nls", LexerOptions::default()).unwrap();
    assert_eq!(offs.len(), toks.len() + 1);
    assert_eq!(offs[toks.len()], 10); // sentinel = input.len()
    // first token starts at 0; the Newline token starts at byte 7; "ls" at 8.
    assert_eq!(offs[0], 0);
    let nl = toks.iter().position(|t| matches!(t, Token::Newline)).unwrap();
    assert_eq!(offs[nl], 7);
    assert_eq!(offs[nl + 1], 8);
}

#[test]
fn offsets_error_returns_failure_position() {
    // Unterminated single quote starting at byte 5.
    let err = tokenize_with_offsets("echo 'oops", LexerOptions::default());
    assert!(err.is_err());
    let (_e, off) = err.unwrap_err();
    assert!(off >= 5, "failure offset {off} should be at/after the open quote");
}

#[test]
fn tokenize_with_opts_output_unchanged() {
    // The wrapper must produce exactly the tokens the offset core produces.
    let a = tokenize_with_opts("if true; then echo hi; fi", LexerOptions::default()).unwrap();
    let (b, _o) = tokenize_with_offsets("if true; then echo hi; fi", LexerOptions::default()).unwrap();
    assert_eq!(a, b);
}
```

Run: `cargo test --bin huck offsets_ 2>&1 | tail` → FAIL (function not defined).

- [ ] **Step 2: Convert the tokenizer core to emit offsets**

First enumerate the emit sites so none are missed:
`grep -n 'tokens.push\|emit_word_with_braces(&mut tokens' src/lexer.rs`
Every one of these must get a paired `offsets.push(<that token's start offset>)`.

In `src/lexer.rs`:

1. Change the current `tokenize_with_opts` into a private core returning tokens
   **and** offsets, with the error carrying the failure byte offset:
   ```rust
   fn tokenize_core(
       input: &str,
       opts: LexerOptions,
   ) -> Result<(Vec<Token>, Vec<usize>), (LexError, usize)> {
   ```
   Build `let mut offsets: Vec<usize> = Vec::new();` next to `tokens`.

   **To capture the failure offset without rewriting every `?` site,** wrap the
   existing tokenizer body in an immediately-invoked closure that still uses `?`
   on `LexError`, then attach `chars.offset()` once on error:
   ```rust
   fn tokenize_core(input: &str, opts: LexerOptions)
       -> Result<(Vec<Token>, Vec<usize>), (LexError, usize)>
   {
       let mut chars = CharCursor::new(input);
       let mut tokens: Vec<Token> = Vec::new();
       let mut offsets: Vec<usize> = Vec::new();
       // ... other locals the old body declared (parts, current, has_token, ...) ...

       let result: Result<(), LexError> = (|| {
           // === the ENTIRE existing tokenizer loop body goes here, unchanged
           //     except: (a) it pushes to `offsets` at each emit (point 3), and
           //     (b) its trailing `Ok(tokens)` becomes `Ok(())`. Internal `?`
           //     and `return Err(..)` keep returning LexError (from the closure).
           Ok(())
       })();

       match result {
           Ok(()) => { offsets.push(input.len()); Ok((tokens, offsets)) }
           Err(e) => Err((e, chars.offset())),
       }
   }
   ```
   The closure captures `&mut chars`, `&mut tokens`, `&mut offsets` and the other
   locals; after it returns, `chars.offset()` is the byte position where lexing
   stopped. (Move the `offsets.push(input.len())` sentinel to the `Ok` arm so it
   is only added on success.)

2. **Maintain a `token_start` byte offset** for the token currently being built:
   - The main loop is `while let Some(c) = chars.next() { … }`. Change it to capture the offset of `c` before consuming:
     ```rust
     loop {
         let c_off = chars.offset();
         let c = match chars.next() { Some(c) => c, None => break };
         …
     }
     ```
   - When a word begins (the `has_token` flag transitions `false -> true`, i.e. the first non-whitespace char of a word is seen), record `token_start = c_off;`.
   - For operator / `Newline` / heredoc tokens emitted directly in the main loop, record their start (`c_off` for a single-char op read in this iteration; for multi-char operators the start is the offset of the first char — capture it when the operator scan begins).

3. **Push an offset for every token pushed.** Every site that does `tokens.push(...)` or calls `emit_word_with_braces(&mut tokens, …)` must push the matching start offset to `offsets`. The cleanest way to keep them in lockstep without auditing 178 call sites by hand is a tiny closure/helper used at the *emit* points:

   - For words: `emit_word_with_braces` is the single word-emit site. Wrap its caller(s) so that immediately after a successful emit you `offsets.push(token_start);`. (There are only a handful of `emit_word_with_braces` call sites — grep `emit_word_with_braces`.)
   - For operators / newline / heredoc: at each `tokens.push(Token::Op(..))` / `tokens.push(Token::Newline)` / heredoc push, add `offsets.push(start)` on the next line, where `start` is the operator/newline start offset captured for that token.

   (The success sentinel `offsets.push(input.len())` lives in the `Ok` arm of
   the `match` in point 1 — do not also push it inside the closure.)

4. Errors are handled by the closure wrapper in point 1 (the closure's `?` /
   `return Err(..)` produce a `LexError`, which the outer `match` pairs with
   `chars.offset()`). No per-`?` rewriting is needed.

5. **Invariant assert** at the end of the core (debug build):
   ```rust
   debug_assert_eq!(offsets.len(), tokens.len() + 1,
       "offset sidecar must have one entry per token plus a sentinel");
   ```

- [ ] **Step 3: Add the public wrappers**

```rust
/// Tokenize, returning each token's start byte offset (and a trailing
/// sentinel `offsets[tokens.len()] == input.len()`). On error, returns the
/// `LexError` and the byte offset where lexing failed.
pub fn tokenize_with_offsets(
    input: &str,
    opts: LexerOptions,
) -> Result<(Vec<Token>, Vec<usize>), (LexError, usize)> {
    tokenize_core(input, opts)
}

pub fn tokenize_with_opts(input: &str, opts: LexerOptions) -> Result<Vec<Token>, LexError> {
    match tokenize_core(input, opts) {
        Ok((tokens, _offsets)) => Ok(tokens),
        Err((e, _off)) => Err(e),
    }
}
```
`tokenize` (the `LexerOptions::default()` wrapper) is unchanged.

- [ ] **Step 4: Run tests**

Run: `cargo test --bin huck 2>&1 | tail -15`
Expected: the three new offset tests pass AND every pre-existing lexer/integration unit test still passes (the wrapper guarantees identical token output). If the lockstep assert trips, find the emit site missing an `offsets.push`.

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat(lexer): tokenize_with_offsets — per-token byte offset sidecar

Single tokenizer core emits a parallel Vec<usize> of token start offsets
(plus an input.len() sentinel) and the byte offset on lex error.
tokenize_with_opts/tokenize now delegate and drop the offsets, so existing
token output is byte-identical. Enables the linear source reader.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Parser — `parse_sequence_opts` + `parse_one_unit`

Let the parser yield one top-level command at a time, stopping at (and consuming) a top-level newline. All existing callers stay byte-identical via a wrapper.

**Files:**
- Modify: `src/command.rs` (`parse_sequence` → wrapper over `parse_sequence_opts`; add the newline-stop arm; add public `parse_one_unit`).

- [ ] **Step 1: Write failing parser tests**

Add to the `#[cfg(test)] mod tests` in `src/command.rs` (use the existing test helpers there; if a `lex`/`tok` helper exists use it, otherwise tokenize inline with `crate::lexer::tokenize`):

```rust
#[test]
fn parse_one_unit_splits_on_top_level_newline() {
    let toks = crate::lexer::tokenize("echo a\necho b\n").unwrap();
    let mut it = toks.into_iter().peekable();
    let u1 = parse_one_unit(&mut it).unwrap().expect("unit 1");
    // first command of unit 1 is `echo a`
    assert!(matches!(u1.first, Command::Simple(_)));
    assert!(u1.rest.is_empty());
    let u2 = parse_one_unit(&mut it).unwrap().expect("unit 2");
    assert!(u2.rest.is_empty());
    assert!(parse_one_unit(&mut it).unwrap().is_none());
}

#[test]
fn parse_one_unit_keeps_semicolon_list_and_andor_together() {
    // `a; b && c` on one line is ONE unit (semicolon and && do not split).
    let toks = crate::lexer::tokenize("a; b && c\n").unwrap();
    let mut it = toks.into_iter().peekable();
    let u = parse_one_unit(&mut it).unwrap().expect("unit");
    assert_eq!(u.rest.len(), 2); // (Semi, b), (And, c)
    assert!(parse_one_unit(&mut it).unwrap().is_none());
}

#[test]
fn parse_one_unit_keeps_multiline_if_as_one_unit() {
    let toks = crate::lexer::tokenize("if true\nthen echo hi\nfi\necho after\n").unwrap();
    let mut it = toks.into_iter().peekable();
    let u1 = parse_one_unit(&mut it).unwrap().expect("if unit");
    assert!(matches!(u1.first, Command::If { .. }));
    let u2 = parse_one_unit(&mut it).unwrap().expect("after unit");
    assert!(matches!(u2.first, Command::Simple(_)));
    assert!(parse_one_unit(&mut it).unwrap().is_none());
}
```
(Adjust the `Command::` variant names to match the real AST — check `enum Command` in `src/command.rs`.)

Run: `cargo test --bin huck parse_one_unit 2>&1 | tail` → FAIL (not defined).

- [ ] **Step 2: Add the `stop_at_top_newline` parameter**

Rename the existing `fn parse_sequence<I>(iter, stop_at)` body to:

```rust
fn parse_sequence_opts<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    stop_at: &[Keyword],
    stop_at_top_newline: bool,
) -> Result<Sequence, ParseError> {
    // ... existing body, with the ONE change in step 3 ...
}

fn parse_sequence<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    stop_at: &[Keyword],
) -> Result<Sequence, ParseError> {
    parse_sequence_opts(iter, stop_at, false)
}
```
All existing `parse_sequence(iter, stop_at)` callers are now byte-identical.

- [ ] **Step 3: Make a top-level newline break the unit**

Inside `parse_sequence_opts`, in the `Token::Op(Operator::Semi) | Token::Newline => { … }` arm, add this as the **first** statement of the arm (before `skip_newlines(iter)`):

```rust
if stop_at_top_newline && matches!(token, Token::Newline) {
    // Unit mode: a top-level newline terminates the command unit (it has
    // already been consumed by the iter.next() above). `;`, `&&`, `||`, `&`
    // and compound-internal newlines are unaffected.
    break;
}
```
(`token` is the variable bound by `let token = iter.next().unwrap();` earlier in the loop.) No other arm changes.

- [ ] **Step 4: Add `parse_one_unit`**

```rust
/// Parse ONE top-level command unit from a pre-tokenized stream, stopping at
/// (and consuming) the next top-level newline or EOF. Skips leading blank-line
/// newlines. Returns `Ok(None)` when only newlines / EOF remain. Used by the
/// non-interactive script reader.
pub fn parse_one_unit<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Option<Sequence>, ParseError> {
    while matches!(iter.peek(), Some(Token::Newline)) {
        iter.next();
    }
    if iter.peek().is_none() {
        return Ok(None);
    }
    Ok(Some(parse_sequence_opts(iter, &[], true)?))
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --bin huck 2>&1 | tail -15`
Expected: the 3 new tests pass; **all** existing parser tests pass (the wrapper preserves behavior).

- [ ] **Step 6: Commit**

```bash
git add src/command.rs
git commit -m "feat(parser): parse_one_unit + stop-at-top-newline mode

parse_sequence becomes a wrapper over parse_sequence_opts(stop_at_top_newline);
when set, a top-level Newline terminates the command unit. parse_one_unit pulls
one top-level command at a time from a token stream for the script reader. All
existing parse_sequence callers are byte-identical.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Rewrite `run_sourced_contents` (the linear reader) + behavior/timing tests

Replace the per-line `classify` loop with tokenize-once / parse-per-unit / re-lex-on-extglob, reconstructing line numbers, `set -v`, and source spans from the offsets. Preserve the existing `ExecOutcome` handling verbatim.

**Files:**
- Modify: `src/builtins.rs` (`run_sourced_contents`, currently `:4960-5048`).
- Create: `tests/linear_source_reader_integration.rs`.

- [ ] **Step 1: Write failing integration tests**

Create `tests/linear_source_reader_integration.rs`. Use the same harness style as `tests/set_x_integration.rs` (a `run(script) -> (stdout, stderr, code)` helper that pipes the script to `huck SCRIPT` via a temp file, or `huck -c`). Copy that helper. Then:

```rust
// Granularity / output equivalence
#[test]
fn semicolon_list_runs_both() {
    let (out, _e, c) = run("echo a; echo b\n");
    assert_eq!(out, "a\nb\n"); assert_eq!(c, 0);
}

#[test]
fn andor_list_short_circuits() {
    let (out, _e, _c) = run("false && echo x; true || echo y\n");
    assert_eq!(out, "y\n");
}

#[test]
fn background_then_foreground() {
    // `true & echo b` backgrounds true, runs echo b in foreground.
    let (out, _e, _c) = run("true & echo b\nwait\n");
    assert!(out.contains('b'));
}

#[test]
fn multiline_if_then_after() {
    let (out, _e, _c) = run("if true\nthen echo hi\nfi\necho after\n");
    assert_eq!(out, "hi\nafter\n");
}

#[test]
fn function_def_then_call() {
    let (out, _e, _c) = run("greet() {\n  echo hello\n}\ngreet\n");
    assert_eq!(out, "hello\n");
}

#[test]
fn heredoc_body_runs() {
    let (out, _e, _c) = run("cat <<EOF\nline1\nline2\nEOF\necho done\n");
    assert_eq!(out, "line1\nline2\ndone\n");
}

// set -v: enabling line not echoed; subsequent lines echoed
#[test]
fn set_v_echoes_subsequent_lines() {
    let (_o, err, _c) = run("set -v\necho one\nset +v\necho two\n");
    // `echo one` echoed (verbose on), `set +v` echoed, `echo two` not.
    assert!(err.contains("echo one"));
    assert!(err.contains("set +v"));
    assert!(!err.contains("echo two"));
    // the enabling `set -v` line itself is not echoed.
    assert!(!err.lines().any(|l| l == "set -v"));
}

// errexit / exit
#[test]
fn errexit_aborts_on_failure() {
    let (out, _e, c) = run("set -e\necho a\nfalse\necho b\n");
    assert_eq!(out, "a\n"); assert_ne!(c, 0);
}

#[test]
fn exit_stops_script() {
    let (out, _e, c) = run("echo a\nexit 3\necho b\n");
    assert_eq!(out, "a\n"); assert_eq!(c, 3);
}

#[test]
fn set_u_unbound_aborts_script() {
    // set -u + reference to an unset var must abort the rest (non-interactive),
    // via the take_pending_fatal_pe_error path preserved in the reader.
    let (out, _e, c) = run("set -u\necho a\necho \"$NOPE_UNDEF_XYZ\"\necho b\n");
    assert_eq!(out, "a\n"); assert_ne!(c, 0);
}

// syntax error: report + continue, correct line number
#[test]
fn syntax_error_reports_line_and_continues() {
    // line 2 is a bare `)` -> syntax error; a and b still print.
    let (out, err, _c) = run("echo a\n)\necho b\n");
    assert!(out.contains('a') && out.contains('b'));
    assert!(err.contains("line 2"), "stderr was: {err}");
}

// mid-file extglob (re-lex remainder)
#[test]
fn midfile_extglob_then_case_pattern() {
    let (out, _e, _c) = run("shopt -s extglob\ncase abc in\n  @(abc|xyz)) echo hit ;;\n  *) echo miss ;;\nesac\n");
    assert_eq!(out, "hit\n");
}

// the bug: a single ~2000-line brace group parses fast (used to take minutes)
#[test]
fn large_single_logical_command_is_fast() {
    use std::time::Instant;
    let mut s = String::from("f() {\n");
    for i in 0..2000 { s.push_str(&format!("  x{i}=$(echo {i})\n")); }
    s.push_str("}\necho built\n");
    let t = Instant::now();
    let (out, _e, c) = run(&s);
    assert_eq!(out, "built\n"); assert_eq!(c, 0);
    assert!(t.elapsed().as_secs() < 5, "took {:?}, expected < 5s (O(n^2) regression)", t.elapsed());
}
```

Run: `cargo test --test linear_source_reader_integration 2>&1 | tail -20`
Expected: most FAIL or hang against the current O(n²) reader (the timing test would time out / be slow). This is the red state.

- [ ] **Step 2: Rewrite `run_sourced_contents`**

Replace the body of `run_sourced_contents` (`src/builtins.rs:4960`) with the following. Keep the `ExecOutcome` match arms **verbatim** from the current implementation (Exit / FunctionReturn / Continue+`take_pending_fatal_pe_error` / LoopBreak|LoopContinue).

```rust
pub(crate) fn run_sourced_contents(
    contents: &str,
    path: &std::path::Path,
    shell: &mut crate::shell_state::Shell,
) -> ExecOutcome {
    let mut last_status = shell.last_status();

    // Line number (1-based) of an absolute byte offset.
    let line_of = |abs: usize| -> usize {
        1 + contents.as_bytes()[..abs.min(contents.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
    };
    let next_line_start = |from: usize| -> usize {
        match contents[from.min(contents.len())..].find('\n') {
            Some(rel) => (from + rel + 1).min(contents.len()),
            None => contents.len(),
        }
    };

    let mut start = 0usize; // byte offset of the unconsumed remainder
    let mut prev_end = 0usize; // bytes already echoed for `set -v`

    'outer: loop {
        if start >= contents.len() {
            break;
        }
        let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
        let (tokens, offsets) = match crate::lexer::tokenize_with_offsets(
            &contents[start..],
            crate::lexer::LexerOptions { extglob },
        ) {
            Ok(t) => t,
            Err((e, fail_off)) => {
                eprintln!(
                    "huck: {}: line {}: syntax error{}",
                    path.display(),
                    line_of(start + fail_off),
                    crate::shell::lex_error_message(e)
                );
                last_status = 2;
                start = next_line_start(start + fail_off);
                prev_end = start;
                continue 'outer;
            }
        };
        let total = tokens.len();
        if total == 0 {
            break;
        }
        let mut iter = tokens.into_iter().peekable();

        loop {
            // Skip blank-line newlines so the unit-start index is accurate.
            while matches!(iter.peek(), Some(crate::lexer::Token::Newline)) {
                iter.next();
            }
            let unit_start_idx = total - iter.len();
            if iter.peek().is_none() {
                start = contents.len();
                break 'outer;
            }
            match crate::command::parse_one_unit(&mut iter) {
                Ok(None) => {
                    start = contents.len();
                    break 'outer;
                }
                Ok(Some(seq)) => {
                    let unit_end_idx = total - iter.len();
                    let unit_start_abs = start + offsets[unit_start_idx];
                    let unit_end_abs = start + offsets[unit_end_idx];

                    if shell.shell_options.verbose {
                        // Echo everything since the previous unit (incl. blank /
                        // comment lines), before executing. Bytes already end in
                        // '\n'; matches the old per-physical-line `set -v` echo.
                        eprint!("{}", &contents[prev_end..unit_end_abs]);
                    }
                    prev_end = unit_end_abs;

                    let span = &contents[unit_start_abs..unit_end_abs];
                    let outcome = crate::executor::execute(&seq, shell, span);

                    match outcome {
                        ExecOutcome::Continue(c) => {
                            last_status = c;
                            if !shell.is_interactive
                                && let Some(st) = shell.take_pending_fatal_pe_error()
                            {
                                return ExecOutcome::Exit(st);
                            }
                        }
                        ExecOutcome::Exit(n) => return ExecOutcome::Exit(n),
                        ExecOutcome::FunctionReturn(n) => {
                            return ExecOutcome::Continue(n);
                        }
                        ExecOutcome::LoopBreak(_, _) | ExecOutcome::LoopContinue(_) => {
                            last_status = 0;
                        }
                    }

                    // Re-lex the remainder if this unit flipped `extglob`.
                    let new_extglob = shell.shopt_options.get("extglob").unwrap_or(false);
                    if new_extglob != extglob {
                        start = unit_end_abs;
                        prev_end = start;
                        continue 'outer;
                    }
                }
                Err(e) => {
                    eprintln!(
                        "huck: {}: line {}: syntax error: {}",
                        path.display(),
                        line_of(start + offsets[unit_start_idx]),
                        crate::shell::parse_error_message(e)
                    );
                    last_status = 2;
                    // Resync: skip tokens up to & incl. the next top-level newline.
                    while let Some(t) = iter.next() {
                        if matches!(t, crate::lexer::Token::Newline) {
                            break;
                        }
                    }
                    prev_end = start + offsets[total - iter.len()];
                }
            }
            if iter.peek().is_none() {
                start = contents.len();
                break 'outer;
            }
        }
    }
    ExecOutcome::Continue(last_status)
}
```

Notes for the implementer:
- Confirm `Token` is importable as `crate::lexer::Token` and `Token::Newline` exists (it does).
- Confirm `executor::execute(&seq, shell, span)` matches the current call's 3rd-arg type (`&str`). The old code passed `&buf`.
- Remove the now-unused `use crate::continuation::{classify, Completeness};` import at the top of the function.
- `Peekable<std::vec::IntoIter<Token>>` implements `ExactSizeIterator`, so `iter.len()` is valid and counts the peeked element as remaining — `total - iter.len()` is the index of the next unconsumed token.

- [ ] **Step 3: Run the new tests + the full suite**

Run: `cargo test --test linear_source_reader_integration 2>&1 | tail -20`
Expected: all pass, including `large_single_logical_command_is_fast` (well under 5s).

Run: `cargo test 2>&1 | tail -20`
Expected: entire suite green (2653+ prior tests + the new ones).

- [ ] **Step 4: Manual nvm.sh smoke check (not committed as a test)**

If `~/.nvm/nvm.sh` exists, verify it now *parses + defines* quickly (stub the final invocation to avoid the unrelated runtime behavior):

```bash
sed '/^nvm_process_parameters "\$@"/d' ~/.nvm/nvm.sh > /tmp/nvm_nocall.sh 2>/dev/null && \
printf '. /tmp/nvm_nocall.sh\necho LOADED\n' > /tmp/drv.sh && \
time timeout 20 ./target/debug/huck /tmp/drv.sh
```
Expected: prints `LOADED` in well under a second (previously: did not finish). If the file is absent, skip.

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs tests/linear_source_reader_integration.rs
git commit -m "feat: linear-time script source reader (M-99)

run_sourced_contents tokenizes the whole script once and parses+executes one
command at a time instead of re-lexing+double-re-parsing the growing buffer per
line via classify. O(n) instead of O(n^2): a single multi-thousand-line { }
logical command (e.g. nvm.sh) now loads instantly. Line numbers, set -v, errexit/
exit/return, set -u abort, mid-file extglob (re-lex remainder), and report+continue
on syntax errors are all preserved via the token offset sidecar. classify and the
interactive REPL reader are untouched.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: bash-diff harness (29th)

Add `tests/scripts/source_reader_diff_check.sh`, modeled on an existing harness (e.g. `tests/scripts/set_x_diff_check.sh`), asserting byte-identical stdout+stderr between bash and huck for script-reader fragments.

**Files:**
- Create: `tests/scripts/source_reader_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Copy the structure of `tests/scripts/set_x_diff_check.sh` (the `check()` helper that runs a fragment through both `bash` and `./target/debug/huck` and diffs combined output, counting pass/fail). Use these fragments (each must be byte-identical between bash and huck):

```
# 1: semicolon list + and-or on one line
printf 'echo a; echo b && echo c\n'
# 2: multi-line if then after
printf 'if true\nthen echo hi\nfi\necho after\n'
# 3: multi-line while
printf 'i=0\nwhile [ $i -lt 3 ]; do\n  echo $i\n  i=$((i+1))\ndone\n'
# 4: case across lines
printf 'case foo in\n  bar) echo no ;;\n  foo) echo yes ;;\nesac\n'
# 5: function def then call
printf 'greet() {\n  echo hello $1\n}\ngreet world\n'
# 6: heredoc
printf 'cat <<EOF\nl1\nl2\nEOF\necho done\n'
# 7: blank + comment lines interspersed
printf 'echo a\n\n# a comment\n\necho b\n'
# 8: mid-file extglob then case pattern
printf 'shopt -s extglob\ncase abc in @(abc|xyz)) echo hit;; *) echo miss;; esac\n'
```

Build first: `cargo build 2>&1 | tail -2`.

- [ ] **Step 2: Run the harness**

Run: `bash tests/scripts/source_reader_diff_check.sh 2>&1 | tail`
Expected: `Total: 8, Pass: 8, Fail: 0`. If a fragment legitimately diverges (e.g. bash-specific stderr wording), either adjust the fragment to a form where they agree or drop it and note why in a comment — do NOT assert on output huck cannot match. Report any dropped fragment.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/source_reader_diff_check.sh
git commit -m "test: bash-diff harness for the linear source reader (29th)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Documentation

Record M-99 as `[fixed v104]`, an `L-` note for the two micro-divergences, a changelog entry, and the README row.

**Files:**
- Modify: `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structure**

Run: `grep -n '^## Change log\|^- \*\*L-21\|Tier 1\|Last updated\|2026-06-06' docs/bash-divergences.md | head` and `grep -n '| v103' README.md`. Read the M-98 entry, the most recent change-log entries, the L-21 note, and the v103 README row to match formatting.

- [ ] **Step 2: Add the M-99 entry**

Add a Tier-1 (bug) entry **M-99 `[fixed v104]`** describing: the O(n²) per-line `classify` cost in `run_sourced_contents`; root cause (whole-buffer re-lex + double-re-parse per physical line, catastrophic for a single multi-thousand-line `{ }` logical command like nvm.sh); the fix (tokenize-once + `parse_one_unit` per command + `CharCursor` byte-offset sidecar + re-lex-on-extglob; `classify` kept for the interactive REPL only). Bump any Tier-1 count.

- [ ] **Step 3: Add the L-22 note**

Add **L-22 `[intentional]`** (next free after L-21) recording the two low divergences from the new reader:
1. A trailing top-level `;` or `&` immediately before a newline (e.g. `set -v ;\ncmd`) groups with the next command into one unit, so a `set -v`/`set +v` taking effect via such a trailing-separator line may echo one fewer/more line than bash. (`set -v ; cmd` on one line already matched bash.)
2. Resync after a syntax error skips to the next top-level newline (token-stream analogue of the old "clear buffer, continue at next line"); the cascade *after* a syntax error may differ slightly from the old per-line resync. Both are negligible and only affect already-divergent-from-bash error/verbose edges.

Bump the L-note count / "Last updated" line.

- [ ] **Step 4: Change-log + README row**

Add a `2026-06-07` v104 change-log entry (style of the v102/v103 entries): the `CharCursor`/`tokenize_with_offsets` sidecar, `parse_one_unit`/stop-at-top-newline, the rewritten `run_sourced_contents` (tokenize-once / parse-per-unit / re-lex-on-extglob), preserved line-numbers/`set -v`/errexit/extglob, 29th harness, the nvm.sh payoff (sourced-script "hang" was O(n²) parse, now O(n) — nvm.sh loads instantly), and the diagnostic credit to v103 `set -x`. Add the v104 README iteration row after v103.

- [ ] **Step 5: Verify + commit**

Run: `grep -n 'M-99\|fixed v104\|L-22\|v104' docs/bash-divergences.md README.md` → confirm real numbers, no placeholders.

```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v104 linear source reader (M-99) — changelog, README, L-22

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final (after all tasks)

- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | tail -5` (full suite green), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] Run every bash-diff harness: `for f in tests/scripts/*_diff_check.sh; do echo "== $f =="; bash "$f" 2>&1 | tail -1; done` (all pass).
- [ ] `huck --rcfile ~/.bashrc` in a pty still loads (if applicable).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files.
