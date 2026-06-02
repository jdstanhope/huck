# huck v78 — Arithmetic For-Loop + Standalone `((expr))` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is implemented by a fresh subagent, with spec-compliance review and code-quality review between tasks.

**Goal:** Add bash's C-style `for ((init; cond; step)) do BODY done` arithmetic for-loop AND bash's standalone `((expr))` command form.

**Architecture:** New lexer token `Token::ArithBlock(String)` captures the raw text between contiguous `((` and matching `))` (depth-tracked, no whitespace inside the opener). Two new `Command` AST variants (`Arith(ArithExpr)`, `ArithFor(Box<ArithForClause>)`) and two new `ParseError` variants. Executor reuses the existing `arith::eval` plus POSIX-for-loop's break/continue/return/exit/SIGINT handling.

**Tech Stack:** Rust 1.85+; existing `src/arith.rs` module (no changes). No new dependencies.

**Branch:** `v78-arith-for` (create from `main` in Preamble P.1).

**Spec:** `docs/superpowers/specs/2026-06-02-huck-arith-for-design.md`.

**Commit trailer (every commit):**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1: Verify clean tree on main**

Run: `git status && git rev-parse --abbrev-ref HEAD`
Expected: branch `main`, clean working tree.

- [ ] **Step 2: Create the iteration branch**

```bash
git checkout -b v78-arith-for
```

Expected: `Switched to a new branch 'v78-arith-for'`.

- [ ] **Step 3: Confirm baseline tests pass**

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "Baseline:", sum}'`
Expected: 2244 (current main).

- [ ] **Step 4: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: `Finished` with no warnings.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/lexer.rs` | New `Token::ArithBlock(String)` variant + new `LexError::UnterminatedArithBlock`. In the `(` arm (around line 396), detect contiguous `((` and invoke a new `scan_arith_block` helper that captures the raw text between `((` and matching `))`. ~8 unit tests | 1 |
| `src/command.rs` | New `Command::Arith(ArithExpr)` and `Command::ArithFor(Box<ArithForClause>)` variants; new `ArithForClause` struct; new `ParseError::ArithBlock(String)` and `ArithForHeader(String)` variants; new `parse_for_command` dispatcher (replaces direct `parse_for` call sites); new `parse_arith_for_clause` + `parse_arith_for_header` + `split_top_level_semi` helpers; new `Token::ArithBlock` dispatch arm in `parse_command`. ~12 unit tests | 1 |
| `src/executor.rs` | New `run_arith` + `run_arith_for`. New match arms in `run_command` for `Command::Arith` and `Command::ArithFor`. ~6 unit tests | 2 |
| `tests/arith_for_integration.rs` | NEW. 8 binary-driven integration tests | 2 |
| `tests/scripts/arith_for_diff_check.sh` | NEW. ~10 bash-diff fragments | 3 |
| `docs/bash-divergences.md` | M-23 flipped to `[fixed v78]`; M-11 entry updated to note `((cmd))` no-space behavior change; new fixed entry for standalone `((expr))`; change-log entry | 3 |
| `README.md` | New v78 iteration row | 3 |

---

## Task 1: Lexer + AST + parser

**Files:**
- Modify: `src/lexer.rs` — add `Token::ArithBlock`, `LexError::UnterminatedArithBlock`, `scan_arith_block` helper, dispatch in the `(` arm
- Modify: `src/command.rs` — add `Command::Arith`, `Command::ArithFor`, `ArithForClause`, `ParseError::{ArithBlock, ArithForHeader}`, `parse_for_command`, `parse_arith_for_clause`, `parse_arith_for_header`, `split_top_level_semi`; new `Token::ArithBlock` dispatch arm in `parse_command`
- Test: 8 lexer tests in `src/lexer.rs::tests`, 12 parser tests in `src/command.rs::tests`

**Goal:** `((1+2))` tokenizes to `Token::ArithBlock("1+2")`; `for ((i=0;i<10;i++)) do :; done` and standalone `((1+2))` both parse to the correct AST. Existing tests continue to pass; the `((cmd))` no-space behavior change (was nested-subshell, now arith) is documented and tested.

### Steps

- [ ] **Step 1: Add `Token::ArithBlock(String)` variant**

Edit `src/lexer.rs`. Find the `pub enum Token` declaration (around line 167). The last variant currently is `Heredoc`. Add:

```rust
pub enum Token {
    Word(Word),
    Op(Operator),
    Newline,
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    /// Raw text inside a `(( ... ))` block (the outer `((` and `))`
    /// already consumed). Parsed by `crate::arith::parse` downstream.
    /// Captured verbatim including embedded `;` separators.
    ArithBlock(String),
}
```

- [ ] **Step 2: Add `LexError::UnterminatedArithBlock` variant**

Find `pub enum LexError` (line 2). Add the variant alongside the existing `Unterminated*` ones:

```rust
pub enum LexError {
    // ... existing variants ...
    /// `((1+2` — EOF before matching `))`.
    UnterminatedArithBlock,
}
```

- [ ] **Step 3: Add the `scan_arith_block` helper**

Place this function near the other scanner helpers in `src/lexer.rs`. Find the section with other `scan_*` helpers (e.g., `scan_backtick_substitution` around line 749 area). Add:

```rust
/// Scans the body of a `(( ... ))` block. The caller has already
/// consumed both opening `(` characters; this function consumes the
/// body and the matching `))`. Returns the raw body text. Tracks
/// nested paren depth so `(((a+b)*c))` correctly captures `((a+b)*c)`
/// as the body.
fn scan_arith_block(
    chars: &mut std::iter::Peekable<std::str::Chars>,
) -> Result<String, LexError> {
    let mut collected = String::new();
    let mut depth: i32 = 0;
    while let Some(c) = chars.next() {
        match c {
            '(' => {
                depth += 1;
                collected.push('(');
            }
            ')' => {
                if depth == 0 && chars.peek() == Some(&')') {
                    chars.next(); // consume the second `)`
                    return Ok(collected);
                }
                depth -= 1;
                collected.push(')');
            }
            _ => collected.push(c),
        }
    }
    Err(LexError::UnterminatedArithBlock)
}
```

- [ ] **Step 4: Detect `((` in the `(` arm of `tokenize`**

Find the `'(' =>` arm in `tokenize` (around line 396):

```rust
            '(' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    emit_word_with_braces(&mut tokens, std::mem::take(&mut parts))?;
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::LParen));
                in_assignment_value = false;
            }
```

Replace it with the version that checks for a contiguous second `(`:

```rust
            '(' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    emit_word_with_braces(&mut tokens, std::mem::take(&mut parts))?;
                    has_token = false;
                }
                // Detect `((` (contiguous, no whitespace). The peek/next
                // sequence below consumes the second `(` only when present.
                // Whitespace between the two `(` is already consumed by the
                // outer loop's whitespace handling — so by the time we get
                // here, a second `(` means they were truly adjacent.
                if chars.peek() == Some(&'(') {
                    chars.next(); // consume the second `(`
                    let body = scan_arith_block(&mut chars)?;
                    tokens.push(Token::ArithBlock(body));
                } else {
                    tokens.push(Token::Op(Operator::LParen));
                }
                in_assignment_value = false;
            }
```

**Note on whitespace disambiguation**: the outer `while let Some(c) = chars.next()` loop's whitespace arm consumes whitespace BEFORE dispatching on the next char. So `( (cmd) )` (with a space between the two `(`s) enters the `(` arm with the first `(`, then the whitespace arm consumes the space, then the next char dispatch enters the `(` arm again with the second `(`. By the time we reach the `chars.peek() == Some(&'(')` check, the peek will see whatever follows the second `(` — NOT the first `(`. So the check correctly distinguishes `((` (adjacent) from `( (` (whitespace-separated).

- [ ] **Step 5: Run baseline tests — confirm nothing broke**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print sum}'`
Expected: 2244 (unchanged — no behavior changes yet from the user's perspective beyond `((cmd))` parsing differently, which no existing test exercises).

If a test DOES fail because it asserted the old `((cmd))` = nested-subshell behavior, locate it. Per the spec, the change is intentional bash-aligning; update the test to either use `( (cmd) )` (with space) for the nested-subshell case or remove/update it as appropriate. Document any such update in the commit message.

- [ ] **Step 6: Add 8 lexer unit tests**

Find the `#[cfg(test)] mod tests` block at the bottom of `src/lexer.rs`. Append these tests:

```rust
    #[test]
    fn arith_block_simple() {
        let tokens = tokenize("((1+2))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s) => assert_eq!(s, "1+2"),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_with_semicolons() {
        let tokens = tokenize("((a;b;c))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s) => assert_eq!(s, "a;b;c"),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_nested_parens() {
        // Outer `((` / `))` is the delimiter; inner parens belong to the body.
        let tokens = tokenize("(((a+b)*c))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s) => assert_eq!(s, "((a+b)*c)"),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_with_internal_whitespace() {
        let tokens = tokenize("((  1 + 2  ))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s) => assert_eq!(s, "  1 + 2  "),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_empty_body() {
        let tokens = tokenize("(())").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s) => assert_eq!(s, ""),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_unclosed_errors() {
        let err = tokenize("((1+2").unwrap_err();
        assert_eq!(err, LexError::UnterminatedArithBlock);
    }

    #[test]
    fn arith_block_single_paren_at_end_errors() {
        // `((1+2)` — one closing paren, not two. Body consumed; depth goes
        // to -1; then EOF → UnterminatedArithBlock.
        let err = tokenize("((1+2)").unwrap_err();
        assert_eq!(err, LexError::UnterminatedArithBlock);
    }

    #[test]
    fn space_between_parens_is_not_arith_block() {
        // `( (cmd) )` — whitespace between the two `(`s. Should tokenize
        // as two LParen ops, a Word, and two RParen ops (nested-subshell
        // path per M-11). The arith-block detector must NOT fire.
        let tokens = tokenize("( (cmd) )").unwrap();
        let lparen_count = tokens
            .iter()
            .filter(|t| matches!(t, Token::Op(Operator::LParen)))
            .count();
        let arith_count = tokens
            .iter()
            .filter(|t| matches!(t, Token::ArithBlock(_)))
            .count();
        assert_eq!(lparen_count, 2, "expected two LParen tokens: {tokens:?}");
        assert_eq!(arith_count, 0, "did not expect ArithBlock: {tokens:?}");
    }
```

- [ ] **Step 7: Run the new lexer tests**

Run: `cargo test --quiet arith_block 2>&1 | tail -10 && cargo test --quiet space_between_parens 2>&1 | tail -5`
Expected: 8 new lexer tests pass.

- [ ] **Step 8: Add the parser AST + error variants**

Edit `src/command.rs`. Find the `pub enum Command` (line 417). Add two new variants:

```rust
pub enum Command {
    // ... existing variants (Pipeline, Simple, If, While, For, Case,
    // BraceGroup, Subshell, FunctionDef, DoubleBracket) ...
    Arith(crate::arith::ArithExpr),
    ArithFor(Box<ArithForClause>),
}
```

Add the `ArithForClause` struct (place it near the existing `ForClause` definition; grep for `pub struct ForClause` to find the location):

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ArithForClause {
    pub init: Option<crate::arith::ArithExpr>,
    pub cond: Option<crate::arith::ArithExpr>,
    pub step: Option<crate::arith::ArithExpr>,
    pub body: Sequence,
}
```

Find the `pub enum ParseError` (line 494). Add two new variants alongside the other `Arith*`/Block-style entries:

```rust
pub enum ParseError {
    // ... existing variants ...
    /// `crate::arith::parse` failed on the body of a `((...))` block
    /// or a for-loop header section. Carries the inner error message.
    ArithBlock(String),
    /// `for ((header))` header did not split into exactly 3
    /// `;`-separated sections.
    ArithForHeader(String),
}
```

- [ ] **Step 9: Add `split_top_level_semi` + `parse_arith_for_header` helpers**

Place these helpers in `src/command.rs`, near the existing `parse_for` helper (around line 947):

```rust
/// Splits `text` on `;` at paren depth 0. Useful for arith-for
/// headers where the body of `for ((init; cond; step))` may contain
/// parenthesized sub-expressions that should not split.
fn split_top_level_semi(text: &str) -> Vec<String> {
    let mut sections: Vec<String> = vec![String::new()];
    let mut depth: i32 = 0;
    for c in text.chars() {
        match c {
            '(' => {
                depth += 1;
                sections.last_mut().unwrap().push(c);
            }
            ')' => {
                depth -= 1;
                sections.last_mut().unwrap().push(c);
            }
            ';' if depth == 0 => sections.push(String::new()),
            _ => sections.last_mut().unwrap().push(c),
        }
    }
    sections
}

/// Splits an arith-for header into three optional arith expressions.
/// Empty sections (e.g., the cond in `((;;))`) yield `None`. Returns
/// `ArithForHeader` if the header doesn't split into exactly 3 sections.
fn parse_arith_for_header(
    text: &str,
) -> Result<
    (Option<crate::arith::ArithExpr>, Option<crate::arith::ArithExpr>, Option<crate::arith::ArithExpr>),
    ParseError,
> {
    let sections = split_top_level_semi(text);
    if sections.len() != 3 {
        return Err(ParseError::ArithForHeader(format!(
            "expected 3 sections separated by `;`, got {}",
            sections.len()
        )));
    }
    let parse_section = |s: &str| -> Result<Option<crate::arith::ArithExpr>, ParseError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            crate::arith::parse(trimmed)
                .map(Some)
                .map_err(|e| ParseError::ArithBlock(e.to_string()))
        }
    };
    Ok((
        parse_section(&sections[0])?,
        parse_section(&sections[1])?,
        parse_section(&sections[2])?,
    ))
}
```

- [ ] **Step 10: Add `parse_arith_for_clause` helper**

In `src/command.rs`, just below `parse_for` (around line 996). The caller has verified the next token is `Token::ArithBlock`; this helper consumes it plus the body:

```rust
/// Parses the body of `for ((header)) [;|newline]* do BODY done`.
/// The caller has consumed `for` and verified the next token is
/// `Token::ArithBlock`. This function consumes the ArithBlock, the
/// separators before `do`, the `do` keyword, the body, and the `done`
/// keyword.
fn parse_arith_for_clause<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<ArithForClause, ParseError> {
    let header_text = match iter.next() {
        Some(Token::ArithBlock(text)) => text,
        _ => unreachable!("caller verified peek"),
    };
    let (init, cond, step) = parse_arith_for_header(&header_text)?;

    // Skip `;` and newline separators between the header and `do`.
    while matches!(
        iter.peek(),
        Some(Token::Op(Operator::Semi)) | Some(Token::Newline)
    ) {
        iter.next();
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;

    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;

    Ok(ArithForClause { init, cond, step, body })
}
```

- [ ] **Step 11: Add `parse_for_command` dispatcher and update call sites**

Currently the parser invokes `parse_for(iter)` directly in two places. We need to dispatch between POSIX and arith for-loop variants. Add a wrapper:

```rust
/// Dispatches `for` to either the POSIX form (`for VAR in WORDS; do ...`)
/// or the bash arith form (`for ((init;cond;step)) do ...`). Called
/// after the `for` keyword has been confirmed (NOT consumed — this
/// function calls `parse_for` or `parse_arith_for_clause` which both
/// expect to consume the `for` keyword themselves).
fn parse_for_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    // Consume the `for` keyword.
    expect_keyword(iter, Keyword::For, ParseError::UnterminatedLoop)?;

    // Peek the next token to choose variant. Skip newlines first so
    // `for\n((...))` works the same as `for ((...))`.
    while matches!(iter.peek(), Some(Token::Newline)) {
        iter.next();
    }

    if matches!(iter.peek(), Some(Token::ArithBlock(_))) {
        return Ok(Command::ArithFor(Box::new(parse_arith_for_clause(iter)?)));
    }

    // POSIX form. parse_for_after_keyword does the rest (the existing
    // parse_for body, but with the `expect_keyword(For)` removed since
    // we already consumed it here).
    Ok(Command::For(Box::new(parse_for_after_keyword(iter)?)))
}
```

Then refactor the existing `parse_for` (line 947): rename it to `parse_for_after_keyword` AND remove its first line (the `expect_keyword(iter, Keyword::For, ParseError::UnterminatedLoop)?;` consume). The rest of the function body is unchanged.

Update the two call sites:
- `src/command.rs:659`: `Some(Keyword::For) => Ok(Command::For(Box::new(parse_for(iter)?)))` becomes `Some(Keyword::For) => parse_for_command(iter)`.
- `src/command.rs:1454`: `Some(Keyword::For) => Ok((Command::For(Box::new(parse_for(iter)?)), false))` becomes `Some(Keyword::For) => Ok((parse_for_command(iter)?, false))`.

- [ ] **Step 12: Add the `Token::ArithBlock` dispatch arm in `parse_command`**

Find `parse_command` in `src/command.rs` (line 647). The match `match iter.peek().and_then(keyword_of)` handles keyword-led commands; the `None => { ... }` arm handles non-keyword starts. We need to intercept `Token::ArithBlock` before either fires.

Add a check at the top of `parse_command`, right after `skip_newlines(iter)`:

```rust
fn parse_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    skip_newlines(iter);

    // Standalone arith block: `((expr))` at command position.
    if matches!(iter.peek(), Some(Token::ArithBlock(_))) {
        let Some(Token::ArithBlock(text)) = iter.next() else { unreachable!() };
        let expr = crate::arith::parse(&text)
            .map_err(|e| ParseError::ArithBlock(e.to_string()))?;
        return Ok(Command::Arith(expr));
    }

    match iter.peek().and_then(keyword_of) {
        // ... existing arms ...
    }
}
```

Also add the same intercept at the top of `parse_command_or_keyword_pipeline` if it exists with a similar shape (grep for it around line 1454 area). If both dispatcher functions exist with their own peek logic, add the intercept in both.

- [ ] **Step 13: Add Command match arms wherever exhaustive matches break**

The new `Command::Arith` and `Command::ArithFor` variants must be handled by any code that exhaustively pattern-matches on `Command`. Run `cargo build` and address each non-exhaustive-match error:

```bash
cargo build 2>&1 | grep -E "non-exhaustive|missing match arm" | head -10
```

Expected sites that need new arms:

**(a) `src/executor.rs::run_command`** (around line 152). Add temporary stub arms — Task 2 Step 3 will replace these with real `run_arith` / `run_arith_for` dispatches:

```rust
        Command::Arith(_) | Command::ArithFor(_) => {
            unimplemented!("Command::Arith and Command::ArithFor wired in Task 2")
        }
```

**(b) `src/command.rs` test helpers** like `first_pipeline` (around line 1806) that exhaustively match Command variants for panic-on-wrong-shape diagnostics. Add arms returning panics with descriptive messages, e.g.:

```rust
            Command::Arith(_) => panic!("expected a pipeline, got an arith command"),
            Command::ArithFor(_) => panic!("expected a pipeline, got an arith for"),
```

**(c) `is_function_body_shape` in `src/command.rs`** (Task 1 of v77 added this). Do **NOT** add arith variants — they are NOT valid function bodies. The function returns `false` for any variant not in its allow-list, which is what we want here.

Iterate until `cargo build` is clean. Parser-level tests added in Step 14 will pass even with the executor unimplemented (parser tests don't run commands).

- [ ] **Step 14: Add 12 parser unit tests**

Append these tests to the existing `#[cfg(test)] mod tests` block at the bottom of `src/command.rs`. Use the idiomatic pattern (`crate::lexer::tokenize(...).unwrap()` + `parse(tokens).unwrap().expect("non-empty parse")`):

```rust
    #[test]
    fn parse_standalone_arith_command() {
        let tokens = crate::lexer::tokenize("((1+2))").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::Arith(_)),
                "got {:?}", parsed.first);
    }

    #[test]
    fn parse_arith_for_full_header() {
        let tokens = crate::lexer::tokenize("for ((i=0;i<10;i++)) do :; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_some());
                assert!(clause.cond.is_some());
                assert!(clause.step.is_some());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_all_empty_sections() {
        let tokens = crate::lexer::tokenize("for ((;;)) do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_none());
                assert!(clause.cond.is_none());
                assert!(clause.step.is_none());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_only_init() {
        let tokens = crate::lexer::tokenize("for ((i=0;;)) do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_some());
                assert!(clause.cond.is_none());
                assert!(clause.step.is_none());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_only_cond() {
        let tokens = crate::lexer::tokenize("for ((;i<10;)) do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_none());
                assert!(clause.cond.is_some());
                assert!(clause.step.is_none());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_only_step() {
        let tokens = crate::lexer::tokenize("for ((;;i++)) do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_none());
                assert!(clause.cond.is_none());
                assert!(clause.step.is_some());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_newline_before_do() {
        let tokens = crate::lexer::tokenize("for ((;;))\ndo break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::ArithFor(_)),
                "got {:?}", parsed.first);
    }

    #[test]
    fn parse_arith_for_semicolon_before_do() {
        let tokens = crate::lexer::tokenize("for ((;;)); do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::ArithFor(_)),
                "got {:?}", parsed.first);
    }

    #[test]
    fn parse_arith_for_two_sections_errors() {
        // `for ((i=0;i<10))` — only one `;`, two sections.
        let tokens = crate::lexer::tokenize("for ((i=0;i<10)) do :; done").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::ArithForHeader(_)), "got {err:?}");
    }

    #[test]
    fn parse_arith_for_bad_arith_in_section_errors() {
        // `for ((i=+;;))` — `i=+` is not a valid arith expression.
        let tokens = crate::lexer::tokenize("for ((i=+;;)) do :; done").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::ArithBlock(_)), "got {err:?}");
    }

    #[test]
    fn parse_arith_for_missing_do_errors() {
        let tokens = crate::lexer::tokenize("for ((;;)) :; done").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::UnterminatedLoop), "got {err:?}");
    }

    #[test]
    fn parse_standalone_arith_with_bad_expr_errors() {
        let tokens = crate::lexer::tokenize("((1++))").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::ArithBlock(_)), "got {err:?}");
    }
```

- [ ] **Step 15: Run the new parser tests**

Run: `cargo test --quiet parse_standalone_arith 2>&1 | tail -5 && cargo test --quiet parse_arith_for 2>&1 | tail -15`
Expected: 12 parser tests pass.

- [ ] **Step 16: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 1:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```

Expected: **2264 tests pass** (2244 baseline + 8 lexer + 12 parser = 2264). Clippy clean.

If `Command::Arith` and `Command::ArithFor` are reported as never-constructed-outside-tests dead-code warnings, that's expected — Task 2 wires the executor and clears these. Confirm warnings are limited to these two variants and not anything else.

- [ ] **Step 17: Parser-only commit; runtime smoke test deferred to Task 2**

No runtime smoke test in this task — Step 13's `unimplemented!()` stub for `Command::Arith` and `Command::ArithFor` in `run_command` will panic if any script actually triggers either variant. Parser-level coverage is verified by the unit tests added in Step 14. Task 2 Step 10 runs the end-to-end smoke test once the executor is wired.

- [ ] **Step 18: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v78 task 1: lexer + AST + parser for `((...))` and `for ((;;))`

* src/lexer.rs: new Token::ArithBlock(String) variant + LexError::
  UnterminatedArithBlock + scan_arith_block helper. The `(` arm in
  tokenize() peeks for a contiguous second `(`; on hit, calls
  scan_arith_block to capture the raw text between `((` and matching
  `))` (depth-tracked). `( (cmd) )` with whitespace continues to
  tokenize as two separate LParen ops (per existing nested-subshell
  behavior).

* src/command.rs: new Command::Arith(ArithExpr) and Command::ArithFor
  (Box<ArithForClause>) variants; new ArithForClause struct; new
  ParseError::ArithBlock(String) and ArithForHeader(String) variants.
  New parse_for_command dispatcher routes `for` to either the POSIX
  parse_for_after_keyword (renamed from parse_for) or the new
  parse_arith_for_clause helper. New parse_arith_for_header +
  split_top_level_semi helpers. New Token::ArithBlock dispatch arm
  at the top of parse_command for standalone use.

Behavior change: `((cmd))` (no space) previously parsed as a nested
subshell per M-11; now parses as arith. `( (cmd) )` (with space)
continues as nested subshell. Documented in v78 spec.

20 new unit tests (8 lexer + 12 parser).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Executor

**Files:**
- Modify: `src/executor.rs` — add `run_arith` + `run_arith_for`; wire into `run_command` match. 6 new unit tests in the existing `#[cfg(test)] mod tests` block.
- Create: `tests/arith_for_integration.rs` — 8 binary-driven integration tests.

**Goal:** Standalone `((expr))` exits 0 if non-zero / 1 if zero; arith-for runs init once, loops on cond, evaluates step, with break/continue/return/exit/SIGINT mirroring the POSIX for-loop.

### Steps

- [ ] **Step 1: Add `run_arith` to `src/executor.rs`**

Find `run_for` (line 294). Add the new helpers below it:

```rust
fn run_arith(expr: &crate::arith::ArithExpr, shell: &mut Shell) -> ExecOutcome {
    match crate::arith::eval(expr, shell) {
        Ok(0) => ExecOutcome::Continue(1),
        Ok(_) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("huck: ((: {e}");
            ExecOutcome::Continue(1)
        }
    }
}
```

- [ ] **Step 2: Add `run_arith_for` to `src/executor.rs`**

Below `run_arith`, add:

```rust
fn run_arith_for(
    clause: &crate::command::ArithForClause,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    use std::sync::atomic::Ordering;

    // 1. Eval init once (if present).
    if let Some(init) = &clause.init {
        if let Err(e) = crate::arith::eval(init, shell) {
            eprintln!("huck: ((: {e}");
            return ExecOutcome::Continue(1);
        }
    }

    let mut last = ExecOutcome::Continue(0);
    loop {
        // SIGINT check (mirrors run_for).
        if shell
            .sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }

        // 2. Eval cond. Empty cond = always true (matches bash).
        let cond_value = match &clause.cond {
            None => 1,
            Some(c) => match crate::arith::eval(c, shell) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("huck: ((: {e}");
                    return ExecOutcome::Continue(1);
                }
            },
        };
        if cond_value == 0 {
            break;
        }

        // 3. Execute body.
        match execute_sequence_body(&clause.body, shell, sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak => {
                last = ExecOutcome::Continue(0);
                break;
            }
            ExecOutcome::LoopContinue => {
                last = ExecOutcome::Continue(0);
                // fall through to step
            }
            ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }

        // 4. Eval step (if present).
        if let Some(step) = &clause.step {
            if let Err(e) = crate::arith::eval(step, shell) {
                eprintln!("huck: ((: {e}");
                return ExecOutcome::Continue(1);
            }
        }
    }
    last
}
```

- [ ] **Step 3: Wire the new helpers into `run_command`**

Find `run_command` (line 152). Add two new match arms alongside the existing variants. If Task 1's Step 13 added panic-arms for `Command::Arith` and `Command::ArithFor`, REPLACE them with real dispatches:

```rust
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        // ... existing arms ...
        Command::For(clause) => run_for(clause, shell, sink),
        Command::ArithFor(clause) => run_arith_for(clause, shell, sink),
        Command::Arith(expr) => run_arith(expr, shell),
        // ... rest ...
    }
}
```

- [ ] **Step 4: Verify the build is clean**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build, no dead-code warnings related to `Command::Arith`/`Command::ArithFor`.

- [ ] **Step 5: Add 6 executor unit tests**

Find the `#[cfg(test)] mod tests` block in `src/executor.rs`. Append these tests. The pattern below uses helper `process_line` to drive end-to-end behavior; check the existing tests in the same module for how they construct a `Shell` and assert on side effects.

Use a helper similar to existing executor tests. If a `process_line_to_outcome(input, &mut shell)` helper or similar exists in the executor's test module, use it. Otherwise build via `crate::shell::process_line(input, shell, false)`.

```rust
    #[test]
    fn arith_command_nonzero_exits_0() {
        let mut sh = Shell::new();
        let outcome = crate::shell::process_line("((1+2))", &mut sh, false);
        assert!(matches!(outcome, ExecOutcome::Continue(0)), "got {outcome:?}");
    }

    #[test]
    fn arith_command_zero_exits_1() {
        let mut sh = Shell::new();
        let outcome = crate::shell::process_line("((0))", &mut sh, false);
        assert!(matches!(outcome, ExecOutcome::Continue(1)), "got {outcome:?}");
    }

    #[test]
    fn arith_command_division_by_zero_exits_1() {
        let mut sh = Shell::new();
        let outcome = crate::shell::process_line("((1/0))", &mut sh, false);
        assert!(matches!(outcome, ExecOutcome::Continue(1)), "got {outcome:?}");
    }

    #[test]
    fn arith_for_counter_loop_sets_var() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for ((i=0;i<3;i++)) do :; done",
            &mut sh,
            false,
        );
        // After the loop, i should be 3 (the value at which cond failed).
        assert_eq!(sh.lookup_var("i").as_deref(), Some("3"));
    }

    #[test]
    fn arith_for_break_stops_at_value() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for ((i=0;i<10;i++)) do if [ $i -eq 5 ]; then break; fi; done",
            &mut sh,
            false,
        );
        // i was 5 when break fired; step does NOT run after break.
        assert_eq!(sh.lookup_var("i").as_deref(), Some("5"));
    }

    #[test]
    fn arith_for_continue_evaluates_step() {
        let mut sh = Shell::new();
        let _ = crate::shell::process_line(
            "for ((i=0;i<5;i++)) do continue; done",
            &mut sh,
            false,
        );
        // i should reach 5 (cond fails) — step runs after continue.
        assert_eq!(sh.lookup_var("i").as_deref(), Some("5"));
    }
```

- [ ] **Step 6: Run the new unit tests**

Run: `cargo test --quiet arith_command 2>&1 | tail -5 && cargo test --quiet arith_for 2>&1 | tail -10`
Expected: 6 executor unit tests pass.

If any test fails because `Shell::lookup_var` returns a different shape (e.g., wraps Variable instead of plain Option<String>), adjust the assertions. Confirm with `grep -n "fn lookup_var" src/shell_state.rs`.

- [ ] **Step 7: Create `tests/arith_for_integration.rs`**

```rust
//! Integration tests for v78 C-style for-loop and standalone arith
//! command. Drives the `huck` binary via stdin and asserts on stdout
//! and exit code.

use std::io::Write;
use std::process::{Command, Stdio};

fn run_huck(script: &str) -> (String, String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn standalone_arith_assignment_persists() {
    let (out, _, code) = run_huck("((x=5))\necho $x\n");
    assert_eq!(code, 0);
    assert_eq!(out, "5\n");
}

#[test]
fn arith_for_counter_prints_each_value() {
    let (out, _, code) = run_huck("for ((i=0;i<5;i++)) do printf '%d ' $i; done\n");
    assert_eq!(code, 0);
    assert_eq!(out, "0 1 2 3 4 ");
}

#[test]
fn arith_for_infinite_with_break_terminates() {
    let (out, _, code) = run_huck("for ((;;)) do break; done\necho ok\n");
    assert_eq!(code, 0);
    assert_eq!(out, "ok\n");
}

#[test]
fn arith_command_in_if_condition() {
    let (out, _, code) = run_huck("if ((5 > 3)); then echo positive; fi\n");
    assert_eq!(code, 0);
    assert_eq!(out, "positive\n");
}

#[test]
fn arith_for_nested() {
    let script = "for ((i=0;i<2;i++)) do for ((j=0;j<2;j++)) do printf '%d%d ' $i $j; done; done\n";
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "00 01 10 11 ");
}

#[test]
fn arith_for_continue_skips_to_step() {
    let script = "for ((i=0;i<5;i++)) do if [ $i -eq 2 ]; then continue; fi; printf '%d ' $i; done\n";
    let (out, _, code) = run_huck(script);
    assert_eq!(code, 0);
    assert_eq!(out, "0 1 3 4 ");
}

#[test]
fn double_paren_no_space_is_arith_not_subshell() {
    // Pre-v78: `((5+5))` parsed as nested subshell (`( (5+5) )`), which
    // would try to run `5+5` as a command and error. Post-v78: arith,
    // exits 0 because the result is non-zero.
    let (_, _, code) = run_huck("((5+5))\n");
    assert_eq!(code, 0, "((5+5)) should be arith and exit 0");
}

#[test]
fn space_between_parens_is_still_subshell() {
    // `( :; )` with whitespace between `(`s continues to parse as
    // nested subshell. The `:` is the null command, exit 0.
    let (_, _, code) = run_huck("( :; )\n");
    assert_eq!(code, 0, "subshell with `:` should exit 0");
}
```

- [ ] **Step 8: Run integration tests**

Run: `cargo test --test arith_for_integration --quiet 2>&1 | tail -10`
Expected: 8 integration tests pass.

If any fail, the most likely cause is a parser-bug uncovered by end-to-end execution. Investigate the diff between expected and actual output.

- [ ] **Step 9: Full suite + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 2:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
```

Expected: **2278 tests pass** (2264 after Task 1 + 6 unit + 8 integration). Clippy clean.

- [ ] **Step 10: Smoke-test from the binary**

```bash
echo 'for ((i=0;i<3;i++)) do echo $i; done' | cargo run --quiet
```

Expected:
```
0
1
2
```

Then:
```bash
echo 'x=10; ((x++)); echo $x' | cargo run --quiet
```

Expected: `11`.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v78 task 2: executor + integration tests for arith for-loop and `((expr))`

* src/executor.rs: run_arith evaluates the expression and exits 0 if
  the result is non-zero, 1 if zero, with a diagnostic to stderr on
  arith error. run_arith_for evaluates init once, loops while cond is
  non-zero (None = always true), evaluates step after each iteration;
  mirrors run_for's break/continue/return/exit/SIGINT handling.

* tests/arith_for_integration.rs (new): 8 binary-driven tests covering
  standalone arith persistence, counter loop, infinite loop with break,
  arith in if-condition, nested arith-for, continue evaluating step,
  the no-space `((cmd))` behavior change, and the space-between-parens
  regression that keeps nested subshells working.

6 new executor unit tests + 8 integration tests = +14 tests. Total
now 2278.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Bash-diff harness + docs

**Files:**
- Create: `tests/scripts/arith_for_diff_check.sh` — ~10 fragments byte-identical to bash 5.2.
- Modify: `docs/bash-divergences.md` — flip M-23 to `[fixed v78]`; update M-11 with the `((cmd))` no-space behavior change note; add change-log entry; refresh Summary table tier count + "Last updated" stamp.
- Modify: `README.md` — v78 iteration row.

**Goal:** End-to-end bash-compat verification; documentation captures the behavior change.

### Steps

- [ ] **Step 1: Create the bash-diff harness**

Create `tests/scripts/arith_for_diff_check.sh` (mirror style from `tests/scripts/function_keyword_diff_check.sh`):

```bash
#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for the C-style arith for-loop
# `for ((init;cond;step)) do BODY done` and the standalone `((expr))`
# command. Each fragment runs through `bash` and `huck` via stdin
# (huck has no -c flag); outputs must be byte-identical.

set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local label="$1"
    local fragment="$2"
    local bash_out huck_out

    bash_out=$(printf '%s\n' "$fragment" | bash 2>&1; echo "EXIT:$?")
    huck_out=$(printf '%s\n' "$fragment" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")

    if [[ "$bash_out" == "$huck_out" ]]; then
        printf "PASS: %s\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s\n" "$label"
        diff <(echo "$bash_out") <(echo "$huck_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# 1. Standalone arith with non-zero result.
check "((1+2)) exit code" \
      '((1+2)); echo $?'

# 2. Standalone arith with zero result.
check "((0)) exit code" \
      '((0)); echo $?'

# 3. Counter loop.
check "for counter" \
      'for ((i=0;i<3;i++)); do echo $i; done'

# 4. Infinite loop with break.
check "for empty header break" \
      'for ((;;)); do break; done; echo ok'

# 5. Continue with step.
check "for continue" \
      'for ((i=0;i<5;i++)); do if [ $i -eq 2 ]; then continue; fi; echo $i; done'

# 6. Arith in if-condition.
check "if arith condition" \
      'if ((5 > 3)); then echo yes; fi'

# 7. Post-increment side effect.
check "post-increment" \
      'x=10; ((x++)); echo $x'

# 8. Nested arith-for loops.
check "nested arith-for" \
      'for ((i=0;i<2;i++)); do for ((j=0;j<2;j++)); do printf "%d%d " $i $j; done; done; echo'

# 9. Zero-result side-effect (assignment of zero exits 1).
check "((x=0)) is exit 1" \
      '((x=0)); echo $?'

# 10. Cond evaluated each iteration (mutable cond).
check "cond re-evaluated" \
      'x=3; for ((;x>0;x--)); do echo $x; done'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
```

Make it executable:

```bash
chmod +x tests/scripts/arith_for_diff_check.sh
```

- [ ] **Step 2: Build and run the harness**

```bash
cargo build --quiet
tests/scripts/arith_for_diff_check.sh
```

Expected: `Total: 10, Pass: 10, Fail: 0`.

If any fragment fails, the diff is shown. Common causes:
- Trailing-newline mismatch (`echo` vs `printf '%s\n'`).
- huck's arith error diagnostic format differs from bash (`huck: ((: ...` vs `bash: ((: ...`). The fragments above shouldn't trigger arith errors; if you add new ones that do, expect a diagnostic-line difference and either work around it (`2>/dev/null`) or skip the fragment.

Iterate until 10/10. If a divergence is intentional (acceptable bash-non-compat), exclude the fragment with a `# DIVERGES: <why>` comment.

- [ ] **Step 3: Update `docs/bash-divergences.md` — flip M-23**

Edit `docs/bash-divergences.md`. Find:

```
- **M-23: C-style `for ((init; cond; step))`** — `[deferred]` medium. huck: parse error. bash: standard counter loop.
```

Replace with:

```
- **M-23: C-style `for ((init; cond; step))`** — `[fixed v78]` medium. New `Token::ArithBlock(String)` in `src/lexer.rs` captures the raw text between contiguous `((` and matching `))` (depth-tracked; no whitespace inside the opener). New `Command::ArithFor(Box<ArithForClause>)` AST variant. New `parse_arith_for_clause` + `parse_arith_for_header` + `split_top_level_semi` helpers in `src/command.rs`. New `run_arith_for` in `src/executor.rs` mirrors `run_for`'s break/continue/return/exit/SIGINT handling. Each header section is `Option<crate::arith::ArithExpr>`; empty cond = always true; `for ((;;))` is a valid infinite loop. Reuses the existing `arith::parse` + `arith::eval` so all v22 arith features (variable references, assignment, post/pre inc/dec, bitwise, etc.) work inside the header. v78 also closes a previously-undocumented gap: standalone `((expr))` as a command — exit 0 if non-zero, 1 if zero, matching bash. The lexer's `((` recognition is shared between both forms.
```

- [ ] **Step 4: Update M-11 with the `((cmd))` no-space behavior change note**

Still in `docs/bash-divergences.md`, find M-11:

```
- **M-11: Subshells `( list )`** — `[fixed (2026-05-26)]` high. Now supported: `(list)` runs the inner sequence in a forked subshell with isolated side effects. ...
```

Append a sentence to the end of the entry:

```
... composition with heredocs/here-strings all work. **v78 update**: contiguous `((cmd))` (no whitespace between the parens) now parses as an arith block per M-23, not as a nested subshell. `( (cmd) )` (with whitespace) continues to parse as nested subshell.
```

- [ ] **Step 5: Add a change-log entry**

Find the change-log section at the bottom of `docs/bash-divergences.md` (it's a chronologically-ordered list of dated entries). Add a new entry after the v77 entry:

```
- **2026-06-02**: M-23 (C-style arith for-loop) shipped as v78. Also closes a previously-undocumented gap: standalone `((expr))` as a command. Shared `Token::ArithBlock(String)` in `src/lexer.rs` captures the raw body between contiguous `((` and matching `))` (depth-tracked). Two new AST variants: `Command::Arith(ArithExpr)` and `Command::ArithFor(Box<ArithForClause>)`. New `run_arith` and `run_arith_for` in `src/executor.rs`; the latter mirrors `run_for`'s break/continue/return/exit/SIGINT handling. Empty header sections (`for ((;;))`) work — empty cond is treated as always-true. Two new `ParseError` variants: `ArithBlock(String)` (carries arith-inner error) and `ArithForHeader(String)` (wrong section count). 20 unit tests + 8 integration tests + 10 bash-diff fragments byte-identical to bash 5.2 (huck's 6th harness). Behavior change documented on M-11: `((cmd))` (no space) now arith-block, not nested subshell. `( (cmd) )` (with space) still nested subshell.
```

- [ ] **Step 6: Refresh the Summary table count + "Last updated" stamp**

At the top of `docs/bash-divergences.md`, find the Summary table that lists tier counts (around lines 20-28). Run:

```bash
grep -c '^- \*\*M-' docs/bash-divergences.md
grep '^- \*\*M-' docs/bash-divergences.md | grep -c '\[deferred\]'
```

Adjust the Tier 2 row to reflect the current state (one M-entry flipped from deferred to fixed: M-23. No new deferreds added; M-09a/M-09b were added in v77's polish). Most likely Tier 2 deferred count decreases by 1.

Find the "Last updated" line (around line 3) and update to `Last updated: 2026-06-02 (after v78 C-style for-loop)`.

- [ ] **Step 7: Update `README.md`**

Edit `README.md`. Find the iteration table. Add a v78 row immediately below the v77 row, matching the existing format. v77's row format is `| v77       | \`function NAME { ... }\` keyword form (M-09)`. So v78 becomes:

```
| v78       | C-style `for ((init;cond;step))` + standalone `((expr))` (M-23) |
```

(Adjust spacing/punctuation to match v77's exact style.)

- [ ] **Step 8: Final full test suite + harness run**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{sum+=$4} END {print "After Task 3:", sum}'
cargo clippy --all-targets 2>&1 | tail -5
cargo build --quiet && tests/scripts/arith_for_diff_check.sh
```

Expected:
- 2278 tests pass (no new tests in Task 3; docs/harness don't add tests).
- Clippy clean.
- Bash-diff harness: 10/10 byte-identical.

Also confirm the four other bash-diff harnesses still pass:

```bash
tests/scripts/arrays_diff_check.sh | tail -1
tests/scripts/ifs_diff_check.sh | tail -1
tests/scripts/test_combinators_diff_check.sh | tail -1
tests/scripts/completion_diff_check.sh | tail -1
tests/scripts/function_keyword_diff_check.sh | tail -1
```

Expected: each prints `Total: N, Pass: N, Fail: 0` for its own fragment count. No regression.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v78 task 3: bash-diff harness + docs

* tests/scripts/arith_for_diff_check.sh (new, +x): huck's 6th
  bash-diff harness, 10 fragments byte-identical to bash 5.2 covering
  standalone arith exit codes, counter loops, empty headers,
  break/continue interaction, mutable cond re-evaluation, nested
  arith-for, post-increment side effects, and the `((x=0)) exit 1`
  case.

* docs/bash-divergences.md:
  - M-23 flipped from [deferred] to [fixed v78] with full
    surface description (token, AST, helpers, executor wiring, empty
    cond semantics).
  - M-11 updated with a v78 parenthetical noting that contiguous
    `((cmd))` now parses as arith-block, not nested subshell;
    `( (cmd) )` with whitespace continues as nested subshell.
  - 2026-06-02 change-log entry added.
  - Summary table tier count + "Last updated" stamp refreshed.

* README.md: v78 iteration row appended below v77.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist

Before merging the branch, the controller dispatches a final code-reviewer over the whole branch diff via `superpowers:requesting-code-review`. Specific things to verify:

- [ ] **All 2278 tests pass on the branch.**
- [ ] **Clippy clean (`cargo clippy --all-targets`).**
- [ ] **The bash-diff harness reports 10/10.**
- [ ] **All other bash-diff harnesses still pass** (arrays, ifs, test_combinators, completion, function_keyword).
- [ ] **`((cmd))` (no space) parses as arith** (integration test `double_paren_no_space_is_arith_not_subshell`).
- [ ] **`( (cmd) )` (with space) still parses as nested subshell** (integration test `space_between_parens_is_still_subshell`).
- [ ] **`for ((;;)) do break; done` works** (all-empty header).
- [ ] **`if ((x > 0)); then ...; fi` works** (arith in if-condition).
- [ ] **`((x++))` mutates `x` visibly** (executor wiring).
- [ ] **Body break/continue/return/exit/SIGINT all propagate correctly through `run_arith_for`** (same as `run_for`).
- [ ] **`docs/bash-divergences.md` M-23 entry is comprehensive and accurate.**
- [ ] **M-11 entry has the v78 update note.**

## Merge

After review fixes land, merge with `--no-ff`:

```bash
git checkout main
git merge --no-ff v78-arith-for -m "Merge v78: C-style for-loop + standalone arith command (M-23)"
git push origin main
git branch -d v78-arith-for
```

Then update the long-running memory files (`huck_iterations.md` + `MEMORY.md`) per the iteration workflow.
