# huck v19: Multi-line Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a huck command span multiple input lines — the REPL reads continuation lines (with a `> ` prompt) until the typed text forms a complete command, which also makes `if`/`while`/`until` work across multiple lines.

**Architecture:** A new `Token::Newline` is emitted by the lexer for newlines outside quotes; the parser treats it as a skippable soft separator, so compound commands parse the same whether written on one line or many. A pure `classify` function (new `src/continuation.rs`) decides whether a buffer is complete, incomplete (and why), or a genuine error. The REPL accumulates physical lines until `classify` reports complete. The AST and executor are untouched.

**Tech Stack:** Rust 2024, `rustyline` 18 (line editor), `expectrl` (PTY tests, dev-dependency).

---

## File Map

| File | Change | Responsibility |
| --- | --- | --- |
| `src/lexer.rs` | Modify | Emit `Token::Newline` for a newline outside quotes |
| `src/command.rs` | Modify | Treat `Newline` as a skippable soft separator; robustness fixes |
| `src/continuation.rs` | Create | Pure `classify` + `joiner_for` — completeness decision |
| `src/main.rs` | Modify | Register `mod continuation;` |
| `src/shell.rs` | Modify | `read_logical_command` continuation loop; PS2; abort; EOF |
| `tests/multiline_integration.rs` | Create | End-to-end piped multi-line scripts |
| `tests/pty_interactive.rs` | Modify | Interactive continuation-prompt and Ctrl-C tests |
| `README.md` | Modify | v19 status row and feature notes |

**Baseline:** `cargo test` passes 679 tests before this work begins.

---

## Task 1: Lexer — the `Newline` token

**Files:**
- Modify: `src/lexer.rs`
- Modify: `src/command.rs` (one-line build fix in `parse_pipeline`)

Adding the `Token::Newline` variant makes `parse_pipeline`'s `match token`
non-exhaustive, so the lexer change and a minimal `parse_pipeline` update
must land together to keep the build compiling. The parser's *real*
newline handling is Task 2.

- [ ] **Step 1: Write the failing lexer tests**

In `src/lexer.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn newline_outside_quotes_emits_newline_token() {
    let tokens = tokenize("a\nb").unwrap();
    assert_eq!(
        tokens,
        vec![
            Token::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
            Token::Newline,
            Token::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
        ]
    );
}

#[test]
fn newline_inside_double_quotes_stays_literal() {
    let tokens = tokenize("\"a\nb\"").unwrap();
    assert_eq!(
        tokens,
        vec![Token::Word(Word(vec![WordPart::Literal {
            text: "a\nb".to_string(),
            quoted: true,
        }]))]
    );
}

#[test]
fn consecutive_newlines_emit_consecutive_tokens() {
    let tokens = tokenize("a\n\nb").unwrap();
    assert_eq!(
        tokens,
        vec![
            Token::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
            Token::Newline,
            Token::Newline,
            Token::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
        ]
    );
}

#[test]
fn carriage_return_is_still_plain_whitespace() {
    // `\r` separates words but does not emit a Newline token.
    let tokens = tokenize("a\rb").unwrap();
    assert_eq!(
        tokens,
        vec![
            Token::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
            Token::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
        ]
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib newline_outside_quotes_emits_newline_token`
Expected: FAIL — compile error, `Token::Newline` does not exist.

- [ ] **Step 3: Add the `Newline` variant and lexer handling, and the `parse_pipeline` build fix**

All three edits land together (the variant addition forces the
`parse_pipeline` edit to compile).

In `src/lexer.rs`, extend the `Token` enum:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(Word),
    Op(Operator),
    Newline,
}
```

In `src/lexer.rs`, in `tokenize`, change the leading whitespace branch.
It currently reads:

```rust
        if c.is_whitespace() {
            if has_token {
                flush_literal(&mut parts, &mut current, false);
                debug_assert!(
                    !parts.is_empty(),
                    "lexer invariant: has_token was true but no parts were emitted"
                );
                tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                has_token = false;
                in_assignment_value = false;
            }
            continue;
        }
```

Change it to also emit a `Newline` token when the whitespace char is `\n`:

```rust
        if c.is_whitespace() {
            if has_token {
                flush_literal(&mut parts, &mut current, false);
                debug_assert!(
                    !parts.is_empty(),
                    "lexer invariant: has_token was true but no parts were emitted"
                );
                tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                has_token = false;
                in_assignment_value = false;
            }
            if c == '\n' {
                tokens.push(Token::Newline);
            }
            continue;
        }
```

In `src/command.rs`, in `parse_pipeline`, the loop's peek-break check
currently reads:

```rust
        if matches!(
            token,
            Token::Op(Operator::Semi | Operator::And | Operator::Or | Operator::Background)
        ) {
            break;
        }
```

Add `Token::Newline` so a newline terminates the pipeline like `;` does:

```rust
        if matches!(
            token,
            Token::Op(Operator::Semi | Operator::And | Operator::Or | Operator::Background)
                | Token::Newline
        ) {
            break;
        }
```

The inner `match token` in `parse_pipeline` is now non-exhaustive. Add a
`Token::Newline` arm right after the `Token::Word(word)` arm:

```rust
            Token::Newline => {
                // Unreachable: the peek-break above stops the loop on a
                // Newline before it is ever consumed here. Task 2 adds a
                // skip_newlines call after the `|` arm; this arm exists
                // only to keep the match exhaustive.
                unreachable!("Newline terminates the pipeline via the peek-break above");
            }
```

- [ ] **Step 4: Run the lexer tests and the full suite**

Run: `cargo test`
Expected: PASS — the 4 new lexer tests pass and all 679 prior tests still
pass (no `\n`-containing string reaches the parser yet, because rustyline
strips line terminators; multi-line input is wired up in Task 4).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs src/command.rs
git commit -m "v19 task 1: Newline token in the lexer"
```

---

## Task 2: Parser — `Newline` as a skippable soft separator

**Files:**
- Modify: `src/command.rs`
- Modify: `src/shell.rs` (message for the new `ParseError::UnexpectedToken`)

`Newline` becomes a soft separator: it ends a command like `;`, but is
*also* skipped wherever a command is expected but absent. This task also
makes the parser panic-free for two pre-existing latent cases the Task 3
classifier would otherwise hit.

- [ ] **Step 1: Write the failing parser tests**

In `src/command.rs`, inside `#[cfg(test)] mod tests`, add. These use the
existing helpers `w_tok`, `kw`, `first_if`, `first_while`.

```rust
#[test]
fn multiline_if_parses_same_as_singleline() {
    // `if a` NL `then b` NL `fi`  ==  `if a ; then b ; fi`
    let multiline = parse(vec![
        kw("if"), w_tok("a"), Token::Newline,
        kw("then"), w_tok("b"), Token::Newline,
        kw("fi"),
    ]).unwrap().unwrap();
    let singleline = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi),
        kw("fi"),
    ]).unwrap().unwrap();
    assert_eq!(multiline, singleline);
}

#[test]
fn newline_after_then_is_skipped() {
    // `if a` NL `then` NL `b` NL `fi`
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Newline,
        kw("then"), Token::Newline,
        w_tok("b"), Token::Newline,
        kw("fi"),
    ]).unwrap().unwrap();
    let clause = first_if(&seq);
    assert_eq!(
        clause.then_body.first,
        Command::Pipeline(Pipeline { commands: vec![plain("b", &[])] })
    );
}

#[test]
fn multiline_while_parses() {
    // `while a` NL `do b` NL `done`
    let seq = parse(vec![
        kw("while"), w_tok("a"), Token::Newline,
        kw("do"), w_tok("b"), Token::Newline,
        kw("done"),
    ]).unwrap().unwrap();
    let clause = first_while(&seq);
    assert!(!clause.until);
    assert_eq!(
        clause.body.first,
        Command::Pipeline(Pipeline { commands: vec![plain("b", &[])] })
    );
}

#[test]
fn newline_separates_top_level_commands() {
    let seq = parse(vec![w_tok("a"), Token::Newline, w_tok("b")])
        .unwrap()
        .unwrap();
    assert_eq!(seq.rest.len(), 1);
    assert_eq!(seq.rest[0].0, Connector::Semi);
}

#[test]
fn leading_newlines_are_skipped() {
    let seq = parse(vec![Token::Newline, Token::Newline, w_tok("a")])
        .unwrap()
        .unwrap();
    assert_eq!(seq.first, Command::Pipeline(Pipeline { commands: vec![plain("a", &[])] }));
}

#[test]
fn all_newline_buffer_is_none() {
    assert_eq!(parse(vec![Token::Newline, Token::Newline]), Ok(None));
}

#[test]
fn newline_after_pipe_continues_pipeline() {
    // `a |` NL `b`  parses as the pipeline `a | b`
    let seq = parse(vec![
        w_tok("a"), Token::Op(Operator::Pipe), Token::Newline, w_tok("b"),
    ]).unwrap().unwrap();
    let p = first_pipeline(&seq);
    assert_eq!(p.commands.len(), 2);
}

#[test]
fn trailing_semicolon_then_newline_is_not_an_error() {
    // `a ;` NL  — the trailing separator run just ends the sequence.
    let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi), Token::Newline])
        .unwrap()
        .unwrap();
    assert_eq!(seq.rest.len(), 0);
}

#[test]
fn then_followed_by_semicolon_still_errors() {
    // A `;` right after `then` is invalid (only a newline is soft there).
    let result = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), Token::Op(Operator::Semi),
        w_tok("b"), Token::Op(Operator::Semi),
        kw("fi"),
    ]);
    assert!(result.is_err());
}

#[test]
fn stray_word_after_compound_errors_without_panic() {
    // `if a; then b; fi extra` — `extra` has no connector before it.
    let result = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi),
        kw("fi"), w_tok("extra"),
    ]);
    assert_eq!(result, Err(ParseError::UnexpectedToken));
}

#[test]
fn if_with_no_body_at_end_of_input_is_unterminated() {
    // `if a` NL `then`  — body not yet typed; must be UnterminatedIf so
    // the classifier treats it as incomplete, not a hard error.
    let result = parse(vec![kw("if"), w_tok("a"), Token::Newline, kw("then")]);
    assert_eq!(result, Err(ParseError::UnterminatedIf));
}

#[test]
fn while_with_no_body_at_end_of_input_is_unterminated() {
    let result = parse(vec![kw("while"), w_tok("a"), Token::Newline, kw("do")]);
    assert_eq!(result, Err(ParseError::UnterminatedLoop));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib multiline_if_parses_same_as_singleline`
Expected: FAIL — `Newline` reaches `parse_sequence`'s `other` arm and
panics via `unreachable!`, and `ParseError::UnexpectedToken` does not exist.

- [ ] **Step 3: Add `ParseError::UnexpectedToken` and the `skip_newlines` helper**

In `src/command.rs`, extend the `ParseError` enum:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
    UnexpectedBackground,
    BackgroundedMultiPipelineSequence,
    UnterminatedIf,
    UnexpectedKeyword(String),
    UnterminatedLoop,
    UnexpectedToken,
}
```

Add a `skip_newlines` helper near `expect_keyword`:

```rust
/// Consumes a run of `Newline` tokens. Newlines are soft separators —
/// they are skipped wherever a command is expected but not yet present.
fn skip_newlines<I: Iterator<Item = Token>>(iter: &mut std::iter::Peekable<I>) {
    while matches!(iter.peek(), Some(Token::Newline)) {
        iter.next();
    }
}
```

- [ ] **Step 4: Update `parse` to skip leading newlines**

Replace the body of `parse`:

```rust
pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    let mut iter = tokens.into_iter().peekable();
    skip_newlines(&mut iter);
    if iter.peek().is_none() {
        return Ok(None);
    }
    let seq = parse_sequence(&mut iter, &[])?;
    Ok(Some(seq))
}
```

- [ ] **Step 5: Skip leading newlines at the start of `parse_command`**

In `parse_command`, add `skip_newlines` as the first statement so a
command body that starts on a fresh line (`then` NL `body`) parses:

```rust
fn parse_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    skip_newlines(iter);
    match iter.peek().and_then(keyword_of) {
        Some(Keyword::If) => Ok(Command::If(Box::new(parse_if(iter)?))),
        Some(Keyword::While) | Some(Keyword::Until) => {
            Ok(Command::While(Box::new(parse_while(iter)?)))
        }
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => Ok(Command::Pipeline(parse_pipeline(iter)?)),
    }
}
```

- [ ] **Step 6: Treat `Newline` as a connector in `parse_sequence`**

In `parse_sequence`, the connector `match token` currently has separate
`Semi`, `And`, `Or` arms and an `other` arm ending in `unreachable!`.
Replace those four arms. The `Semi` arm gains `Token::Newline` and a
`skip_newlines` call; `And`/`Or` gain `skip_newlines`; `other` returns an
error instead of panicking:

```rust
            Token::Op(Operator::Semi) | Token::Newline => {
                skip_newlines(iter);
                match iter.peek() {
                    None => break,
                    Some(tok) => {
                        if keyword_of(tok).map(|k| stop_at.contains(&k)).unwrap_or(false) {
                            break;
                        }
                    }
                }
                rest.push((Connector::Semi, parse_command(iter)?));
            }
            Token::Op(Operator::And) => {
                skip_newlines(iter);
                rest.push((Connector::And, parse_command(iter)?));
            }
            Token::Op(Operator::Or) => {
                skip_newlines(iter);
                rest.push((Connector::Or, parse_command(iter)?));
            }
            other => {
                if let Some(kw) = keyword_of(&other) {
                    return Err(ParseError::UnexpectedKeyword(kw.name().to_string()));
                }
                // A non-keyword, non-connector token after a command —
                // e.g. a stray word or `|` after a closed `if`/`while`.
                return Err(ParseError::UnexpectedToken);
            }
```

The loop's top-of-loop peek logic is unchanged: `keyword_of` returns
`None` for a `Newline`, so a `Newline` is never mistaken for a `stop_at`
keyword and falls through to be consumed as a connector.

- [ ] **Step 7: Continue a pipeline across a newline after `|`**

In `src/command.rs`, `parse_pipeline`'s `Token::Op(Operator::Pipe)` arm
currently reads:

```rust
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(finalize_stage(
                    prog,
                    std::mem::take(&mut args),
                    stdin.take(),
                    stdout.take(),
                    stderr.take(),
                ));
            }
```

Add a `skip_newlines` call at the end of the arm so a newline right
after `|` is consumed rather than terminating the pipeline (`cmd |`⏎`cmd`
continues):

```rust
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(finalize_stage(
                    prog,
                    std::mem::take(&mut args),
                    stdin.take(),
                    stdout.take(),
                    stderr.take(),
                ));
                skip_newlines(iter);
            }
```

The `Token::Newline => unreachable!(...)` arm added in Task 1 stays
correct: a `Newline` that is *not* directly after `|` is caught by the
loop's peek-break, and one that *is* directly after `|` is consumed by
this `skip_newlines`, so the inner `match token` never receives a
`Newline`.

- [ ] **Step 8: Map "ran out of input" to the compound's unterminated error**

A compound command whose condition or body has not been typed yet must
report `UnterminatedIf`/`UnterminatedLoop` (so the classifier sees
"incomplete"), not `MissingCommand`. Add a helper near `parse_if`:

```rust
/// Runs `parse_sequence` for a compound command's condition or body.
/// If it fails with `MissingCommand` because input simply ran out
/// (the iterator is exhausted), the failure is the compound command
/// being unterminated — report `unterminated` instead. A
/// `MissingCommand` with tokens still pending is a genuine error and
/// passes through unchanged.
fn parse_compound_section<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    stop_at: &[Keyword],
    unterminated: ParseError,
) -> Result<Sequence, ParseError> {
    match parse_sequence(iter, stop_at) {
        Err(ParseError::MissingCommand) if iter.peek().is_none() => Err(unterminated),
        other => other,
    }
}
```

In `parse_if`, replace the three `parse_sequence(...)` calls with
`parse_compound_section(...)`:

```rust
fn parse_if<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<IfClause, ParseError> {
    expect_keyword(iter, Keyword::If, ParseError::UnterminatedIf)?;
    let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
    expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
    let then_body = parse_compound_section(
        iter,
        &[Keyword::Elif, Keyword::Else, Keyword::Fi],
        ParseError::UnterminatedIf,
    )?;

    let mut elif_branches = Vec::new();
    while iter.peek().and_then(keyword_of) == Some(Keyword::Elif) {
        iter.next(); // consume `elif`
        let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
        expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
        let body = parse_compound_section(
            iter,
            &[Keyword::Elif, Keyword::Else, Keyword::Fi],
            ParseError::UnterminatedIf,
        )?;
        elif_branches.push(ElifBranch { condition, body });
    }

    let else_body = if iter.peek().and_then(keyword_of) == Some(Keyword::Else) {
        iter.next(); // consume `else`
        Some(parse_compound_section(iter, &[Keyword::Fi], ParseError::UnterminatedIf)?)
    } else {
        None
    };

    expect_keyword(iter, Keyword::Fi, ParseError::UnterminatedIf)?;
    Ok(IfClause { condition, then_body, elif_branches, else_body })
}
```

In `parse_while`, replace its two `parse_sequence(...)` calls:

```rust
fn parse_while<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<WhileClause, ParseError> {
    let until = match iter.next().as_ref().and_then(keyword_of) {
        Some(Keyword::While) => false,
        Some(Keyword::Until) => true,
        _ => unreachable!("parse_command guarantees a while/until keyword here"),
    };
    let condition = parse_compound_section(iter, &[Keyword::Do], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;
    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(WhileClause { condition, body, until })
}
```

- [ ] **Step 9: Add the `UnexpectedToken` message in `shell.rs`**

In `src/shell.rs`, `parse_error_message` has a `match` over `ParseError`.
Add an arm (the function returns a message with no leading colon — the
`": "` is supplied by the caller's format string):

```rust
        ParseError::UnexpectedToken => "unexpected token after command".to_string(),
```

- [ ] **Step 10: Run the tests**

Run: `cargo test`
Expected: PASS — all 12 new parser tests pass, and all prior tests
(including the v17/v18 single-line `if`/`while` suites) still pass. The
v17/v18 suites passing is the proof the retrofit is backward-compatible.

- [ ] **Step 11: Commit**

```bash
git add src/command.rs src/shell.rs
git commit -m "v19 task 2: parser treats Newline as a soft separator"
```

---

## Task 3: Completeness classifier

**Files:**
- Create: `src/continuation.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod continuation;` in alphabetical position
(between `mod command;` and `mod executor;`):

```rust
mod arith;
mod builtins;
mod command;
mod continuation;
mod executor;
mod expand;
mod history;
mod job_spec;
mod jobs;
mod lexer;
mod param_expansion;
mod shell;
mod shell_state;
mod test_builtin;
```

- [ ] **Step 2: Write the failing classifier tests**

Create `src/continuation.rs` with only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_simple_command() {
        assert_eq!(classify("echo hi"), Completeness::Complete);
    }

    #[test]
    fn complete_multiline_if() {
        assert_eq!(classify("if true\nthen echo hi\nfi"), Completeness::Complete);
    }

    #[test]
    fn empty_buffer_is_complete() {
        assert_eq!(classify(""), Completeness::Complete);
    }

    #[test]
    fn open_double_quote_is_incomplete() {
        assert_eq!(
            classify("echo \"hello"),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn open_command_substitution_is_incomplete() {
        assert_eq!(
            classify("echo $(date"),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }

    #[test]
    fn trailing_pipe_is_incomplete() {
        assert_eq!(
            classify("echo hi |"),
            Completeness::Incomplete(ContinuationReason::Operator)
        );
    }

    #[test]
    fn trailing_andand_is_incomplete() {
        assert_eq!(
            classify("echo hi &&"),
            Completeness::Incomplete(ContinuationReason::Operator)
        );
    }

    #[test]
    fn unterminated_if_is_incomplete() {
        assert_eq!(
            classify("if true"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn unterminated_while_is_incomplete() {
        assert_eq!(
            classify("while true\ndo echo hi"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn if_awaiting_body_is_incomplete() {
        assert_eq!(
            classify("if true\nthen"),
            Completeness::Incomplete(ContinuationReason::Compound)
        );
    }

    #[test]
    fn trailing_backslash_is_incomplete() {
        assert_eq!(
            classify("echo hi \\"),
            Completeness::Incomplete(ContinuationReason::Backslash)
        );
    }

    #[test]
    fn even_trailing_backslashes_are_not_a_continuation() {
        // `\\` is an escaped backslash — the line is complete.
        assert_eq!(classify("echo hi\\\\"), Completeness::Complete);
    }

    #[test]
    fn genuine_syntax_error_is_error() {
        // A stray `)` is a lexer/parser error, not an incompletion.
        assert_eq!(classify("echo hi | | grep x"), Completeness::Error);
    }

    #[test]
    fn stray_word_after_fi_is_error() {
        assert_eq!(classify("if true; then echo; fi extra"), Completeness::Error);
    }

    #[test]
    fn joiner_backslash_is_empty() {
        assert_eq!(joiner_for(ContinuationReason::Backslash, "echo a"), "");
    }

    #[test]
    fn joiner_operator_is_space() {
        assert_eq!(joiner_for(ContinuationReason::Operator, "echo a |"), " ");
    }

    #[test]
    fn joiner_open_quote_is_semicolon() {
        assert_eq!(joiner_for(ContinuationReason::OpenQuote, "echo \"a"), "; ");
    }

    #[test]
    fn joiner_compound_is_semicolon_after_a_command() {
        assert_eq!(joiner_for(ContinuationReason::Compound, "if true"), "; ");
    }

    #[test]
    fn joiner_compound_is_space_after_a_bare_keyword() {
        assert_eq!(joiner_for(ContinuationReason::Compound, "then"), " ");
        assert_eq!(joiner_for(ContinuationReason::Compound, "  do  "), " ");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib continuation`
Expected: FAIL — compile error, `classify`/`Completeness`/etc. undefined.

- [ ] **Step 4: Implement the classifier**

At the top of `src/continuation.rs` (above the test module), add:

```rust
//! Decides whether a typed buffer forms a complete command, needs more
//! input (and why), or is a genuine syntax error. Pure — it runs the
//! real lexer and parser and classifies the outcome, so it can never
//! disagree with them.

use crate::command::{self, ParseError};
use crate::lexer::{self, LexError, Operator, Token};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ContinuationReason {
    Backslash,
    OpenQuote,
    Operator,
    Compound,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Completeness {
    Complete,
    Incomplete(ContinuationReason),
    Error,
}

/// True when `s` ends with an odd-length run of backslashes — the final
/// backslash is an unescaped line-continuation marker.
fn ends_with_continuation_backslash(s: &str) -> bool {
    s.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

/// The lexer errors that mean "a quote or expansion is still open" — as
/// opposed to a malformed-but-closed construct.
fn is_unterminated_lex(e: &LexError) -> bool {
    matches!(
        e,
        LexError::UnterminatedQuote
            | LexError::UnterminatedBrace
            | LexError::UnterminatedSubstitution
            | LexError::UnterminatedArith
    )
}

/// Classifies `buffer`. See module docs.
pub fn classify(buffer: &str) -> Completeness {
    if ends_with_continuation_backslash(buffer) {
        return Completeness::Incomplete(ContinuationReason::Backslash);
    }
    let tokens = match lexer::tokenize(buffer) {
        Ok(tokens) => tokens,
        Err(e) => {
            return if is_unterminated_lex(&e) {
                Completeness::Incomplete(ContinuationReason::OpenQuote)
            } else {
                Completeness::Error
            };
        }
    };
    if matches!(
        tokens.last(),
        Some(Token::Op(Operator::Pipe | Operator::And | Operator::Or))
    ) {
        return Completeness::Incomplete(ContinuationReason::Operator);
    }
    match command::parse(tokens) {
        Ok(_) => Completeness::Complete,
        Err(ParseError::UnterminatedIf | ParseError::UnterminatedLoop) => {
            Completeness::Incomplete(ContinuationReason::Compound)
        }
        Err(_) => Completeness::Error,
    }
}

/// True when `line`'s last whitespace-delimited word is a control
/// keyword after which a `;` would be invalid.
fn ends_with_control_keyword(line: &str) -> bool {
    matches!(
        line.split_whitespace().next_back(),
        Some("if" | "while" | "until" | "then" | "do" | "else" | "elif")
    )
}

/// The separator to splice before a continuation line when collapsing a
/// multi-line command into its single-line history form. `last_line` is
/// the line that triggered the continuation.
pub fn joiner_for(reason: ContinuationReason, last_line: &str) -> &'static str {
    match reason {
        ContinuationReason::Backslash => "",
        ContinuationReason::Operator => " ",
        ContinuationReason::OpenQuote => "; ",
        ContinuationReason::Compound => {
            if ends_with_control_keyword(last_line) {
                " "
            } else {
                "; "
            }
        }
    }
}
```

Note: `classify("")` tokenizes to an empty token vector, which
`command::parse` maps to `Ok(None)` → `Completeness::Complete`.

- [ ] **Step 5: Run the tests**

Run: `cargo test --lib continuation`
Expected: PASS — all 19 classifier tests pass.

- [ ] **Step 6: Run the full suite**

Run: `cargo test`
Expected: PASS — all prior tests plus the new module's tests.

- [ ] **Step 7: Commit**

```bash
git add src/continuation.rs src/main.rs
git commit -m "v19 task 3: completeness classifier"
```

---

## Task 4: REPL continuation loop

**Files:**
- Modify: `src/shell.rs`

The REPL reads physical lines, accumulating two strings — `buffer` (fed
to the executor, joined with real newlines) and `history_entry` (stored,
joined with context-appropriate separators) — until `classify` reports
the buffer is `Complete` or a genuine `Error`.

- [ ] **Step 1: Add the continuation-prompt constant and the `ReadResult` type**

In `src/shell.rs`, below the existing `const PROMPT`, add:

```rust
const CONT_PROMPT: &str = "> ";
```

Above `fn run()`, add the result type:

```rust
/// The outcome of reading one logical (possibly multi-line) command.
enum ReadResult {
    /// A finished command: `buffer` is fed to the executor, `history`
    /// is its single-line form for the history list.
    Ready { buffer: String, history: String },
    /// Ctrl-C — any partial command is discarded; the REPL loops.
    Interrupted,
    /// Ctrl-D at an empty first-line prompt — exit the shell cleanly.
    Eof,
    /// EOF while a partial command was pending — a truncated command.
    EofMidCommand,
    /// A rustyline read error — exit the shell.
    ReadError(String),
}
```

- [ ] **Step 2: Implement `read_logical_command`**

Add this function below `run()` in `src/shell.rs`:

```rust
/// Reads one logical command, gathering continuation lines until the
/// accumulated buffer classifies as `Complete` or a genuine `Error`.
fn read_logical_command(
    editor: &mut Editor<HuckHelper, FileHistory>,
    shell: &mut Shell,
) -> ReadResult {
    use crate::continuation::{classify, joiner_for, Completeness};

    let mut buffer = String::new();
    let mut history = String::new();
    // The reason the buffer-so-far is incomplete, and the line that
    // caused it — together they pick the joiner for the next line.
    let mut pending: Option<(crate::continuation::ContinuationReason, String)> = None;

    loop {
        let prompt = if pending.is_none() { PROMPT } else { CONT_PROMPT };
        match editor.readline(prompt) {
            Ok(raw) => {
                // History expansion runs per physical line, as before.
                let line = match crate::history::expand(&raw, &shell.history) {
                    Ok(None) => raw,
                    Ok(Some(expanded)) => {
                        println!("{expanded}");
                        expanded
                    }
                    Err(e) => {
                        eprintln!("huck: {e}");
                        shell.set_last_status(1);
                        return ReadResult::Interrupted;
                    }
                };

                match pending.take() {
                    None => {
                        // First physical line.
                        buffer.push_str(&line);
                        history.push_str(&line);
                    }
                    Some((reason, prev_line)) => {
                        // `buffer` joins with a real newline, except a
                        // backslash continuation which joins with nothing.
                        if reason != crate::continuation::ContinuationReason::Backslash {
                            buffer.push('\n');
                        }
                        buffer.push_str(&line);
                        history.push_str(joiner_for(reason, &prev_line));
                        history.push_str(&line);
                    }
                }

                match classify(&buffer) {
                    Completeness::Complete | Completeness::Error => {
                        return ReadResult::Ready { buffer, history };
                    }
                    Completeness::Incomplete(reason) => {
                        if reason == crate::continuation::ContinuationReason::Backslash {
                            // Drop the unescaped trailing backslash from
                            // both accumulators before the next line.
                            buffer.pop();
                            history.pop();
                        }
                        pending = Some((reason, line));
                    }
                }
            }
            Err(ReadlineError::Interrupted) => return ReadResult::Interrupted,
            Err(ReadlineError::Eof) => {
                return if buffer.is_empty() {
                    ReadResult::Eof
                } else {
                    ReadResult::EofMidCommand
                };
            }
            Err(e) => return ReadResult::ReadError(e.to_string()),
        }
    }
}
```

- [ ] **Step 3: Rewrite the `run()` loop body to use `read_logical_command`**

In `run()`, the loop currently reads `editor.readline(PROMPT)` directly
and inlines history expansion and history adding. Replace the whole
`loop { ... }` body (the `loop` starting after the history-load block)
with:

```rust
    loop {
        crate::jobs::reap_and_notify(&mut shell);
        if let Some(helper) = editor.helper_mut() {
            helper.refresh(&shell);
        }
        match read_logical_command(&mut editor, &mut shell) {
            ReadResult::Ready { buffer, history } => {
                if !history.trim().is_empty() {
                    shell.history.add(history.clone());
                    let _ = editor.add_history_entry(history.as_str());
                }
                match process_line(&buffer, &mut shell) {
                    ExecOutcome::Exit(code) => {
                        shell.history.save();
                        return code;
                    }
                    ExecOutcome::Continue(status) => shell.set_last_status(status),
                    ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => {
                        shell.set_last_status(0)
                    }
                }
            }
            ReadResult::Interrupted => continue,
            ReadResult::Eof => {
                shell.history.save();
                return shell.last_status();
            }
            ReadResult::EofMidCommand => {
                eprintln!("huck: syntax error: unexpected end of input");
                shell.history.save();
                return 2;
            }
            ReadResult::ReadError(msg) => {
                eprintln!("huck: input error: {msg}");
                return 1;
            }
        }
    }
```

The `Editor`/`FileHistory`/`HuckHelper` types are already imported at the
top of `src/shell.rs`; `read_logical_command`'s signature uses them
directly.

- [ ] **Step 4: Build and run the full suite**

Run: `cargo test`
Expected: PASS — all prior tests still pass. The single-line behavior is
unchanged: a complete first line classifies as `Complete` immediately and
runs exactly as before.

- [ ] **Step 5: Manual smoke test**

```bash
cargo build --release
./target/release/huck <<'EOF'
if true
then
echo multi-line-if
fi
i=0
while test $i -lt 2
do
echo loop-$i
i=$((i+1))
done
echo one \
two \
three
echo a |
cat
history
EOF
```

Expected output, in order: `multi-line-if`; `loop-0`; `loop-1`;
`echo one two three`'s output `one two three`; `a`; then the `history`
listing showing the multi-line `if` as `if true; then echo multi-line-if; fi`,
the `while` as `while test $i -lt 2; do echo loop-$i; i=$((i+1)); done`,
and `echo one two three` and `echo a | cat` on single lines. Report the
actual output.

Also test interactively that an incomplete line shows `> `:

```bash
printf 'if true\nthen echo X\nfi\nexit\n' | ./target/release/huck
```

Expected: prints `X`. Report the actual output.

- [ ] **Step 6: Commit**

```bash
git add src/shell.rs
git commit -m "v19 task 4: REPL continuation loop for multi-line input"
```

---

## Task 5: End-to-end integration tests

**Files:**
- Create: `tests/multiline_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/multiline_integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with `script` piped to stdin; returns (stdout, stderr).
fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Runs huck and also returns the decoded exit status code.
fn run_with_status(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn multiline_if() {
    let (out, _) = run("if true\nthen\necho yes\nfi\nexit\n");
    assert!(out.lines().any(|l| l == "yes"), "stdout: {out}");
}

#[test]
fn multiline_if_else_taken() {
    let (out, _) = run("if false\nthen\necho a\nelse\necho b\nfi\nexit\n");
    assert!(out.lines().any(|l| l == "b"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "a"), "stdout: {out}");
}

#[test]
fn multiline_while() {
    let (out, _) = run("i=0\nwhile test $i -lt 3\ndo\necho n$i\ni=$((i+1))\ndone\nexit\n");
    for marker in ["n0", "n1", "n2"] {
        assert!(out.lines().any(|l| l == marker), "missing {marker}: {out}");
    }
}

#[test]
fn multiline_until() {
    let (out, _) = run("n=2\nuntil test $n -eq 0\ndo\necho u$n\nn=$((n-1))\ndone\nexit\n");
    assert!(out.lines().any(|l| l == "u2"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "u1"), "stdout: {out}");
}

#[test]
fn nested_loop_inside_if() {
    let script = "if true\nthen\ni=0\nwhile test $i -lt 2\ndo\necho x$i\ni=$((i+1))\ndone\nfi\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "x0"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "x1"), "stdout: {out}");
}

#[test]
fn quote_spanning_two_lines() {
    // The newline inside the quote is literal content.
    let (out, _) = run("echo \"line one\nline two\"\nexit\n");
    assert!(out.contains("line one\nline two"), "stdout: {out:?}");
}

#[test]
fn trailing_pipe_continues() {
    let (out, _) = run("echo hello |\ncat\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out}");
}

#[test]
fn trailing_andand_continues() {
    let (out, _) = run("true &&\necho reached\nexit\n");
    assert!(out.lines().any(|l| l == "reached"), "stdout: {out}");
}

#[test]
fn backslash_newline_joins_lines() {
    let (out, _) = run("echo one \\\ntwo \\\nthree\nexit\n");
    assert!(out.lines().any(|l| l == "one two three"), "stdout: {out}");
}

#[test]
fn eof_inside_unterminated_if_is_a_syntax_error() {
    let (_, err, code) = run_with_status("if true\nthen\necho hi\n");
    assert!(
        err.to_lowercase().contains("unexpected end of input"),
        "stderr: {err}"
    );
    assert_eq!(code, 2, "exit code");
}

#[test]
fn multiline_command_stored_as_single_history_line() {
    // After a multi-line `if`, `history` lists it collapsed onto one line.
    let (out, _) = run("if true\nthen\necho hi\nfi\nhistory\nexit\n");
    assert!(
        out.lines().any(|l| l.contains("if true; then echo hi; fi")),
        "history did not show the collapsed form; stdout: {out}"
    );
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test multiline_integration`
Expected: 11 tests pass.

If a test fails, inspect the actual stdout/stderr. Multi-line behavior
was verified in Tasks 1-4; only adjust an assertion if it genuinely
mismatches correct output — never change shell behavior to fit a test.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: PASS — all tests, prior plus the 11 new.

- [ ] **Step 4: Commit**

```bash
git add tests/multiline_integration.rs
git commit -m "v19 task 5: end-to-end multi-line input integration tests"
```

---

## Task 6: PTY interactive tests

**Files:**
- Modify: `tests/pty_interactive.rs`

- [ ] **Step 1: Add the continuation-prompt tests**

In `tests/pty_interactive.rs`, append three tests at the end of the file
(before the closing of the file — these are top-level `#[test]`
functions). They reuse the existing helpers `try_spawn`, `send`,
`expect`, `expect_eof`, `histfile_env`, `env_refs`, `settle`, and the
`ENTER`/`CTRL_C` constants.

```rust
#[test]
fn pty_continuation_prompt_appears() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // An unterminated `if` must draw the `> ` continuation prompt.
    send(&mut session, "if true");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_multiline_if_runs() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "if true");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "then echo MARKER42");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "fi");
    send(&mut session, ENTER);
    // The body runs only if the three lines were assembled into one
    // complete `if` command.
    expect(&mut session, "MARKER42");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_ctrl_c_aborts_multiline_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    // Start a multi-line `if`, then abort it with Ctrl-C.
    send(&mut session, "if true");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    settle();
    send(&mut session, CTRL_C);
    // After the abort the main prompt returns and the partial command
    // is gone — a fresh `pwd` runs alone and prints the temp dir name.
    expect(&mut session, "huck> ");
    send(&mut session, "pwd");
    send(&mut session, ENTER);
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    expect(&mut session, marker);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
```

- [ ] **Step 2: Run the PTY tests**

Run: `cargo test --test pty_interactive`
Expected: PASS — the 3 new tests pass (or log a skip notice and pass if
no PTY is available, like the existing PTY tests).

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: PASS — all tests.

- [ ] **Step 4: Commit**

```bash
git add tests/pty_interactive.rs
git commit -m "v19 task 6: PTY tests for the continuation prompt"
```

---

## Task 7: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v19 row to the status table**

Append after the v18 row:

```
| v19       | Multi-line input (continuation lines, `> ` prompt)      |
```

Match the table's column alignment.

- [ ] **Step 2: Add a feature note**

After the v18 `while`/`until` feature block, add:

```markdown
**Multi-line input (v19):**
A command can span several input lines. The REPL reads continuation
lines — showing a `> ` prompt — until the typed text forms a complete
command: an unterminated `if`/`while`/`until`, an open quote or
expansion (`'`, `"`, `` ` ``, `$(`, `${`, `$((`), a pending operator
(`|`, `&&`, `||`), or a line ending in a backslash all carry over onto
the next line. `if`/`while`/`until` can therefore be written across
multiple lines, the way they appear in scripts. Ctrl-C at the `> `
prompt discards the partial command; an EOF mid-command is a syntax
error. A multi-line command is stored in history collapsed onto one
line.
```

- [ ] **Step 3: Drop the "single-line form only" caveats from v17 and v18**

In the `**`if` control flow (v17):**` block, the text currently ends:
`Single-line form only (parts separated by `;`); multi-line `if`, `if`
inside a `|` pipeline, and backgrounding a whole `if` are not yet
implemented.` Replace that sentence with:
``if` inside a `|` pipeline and backgrounding a whole `if` are not yet
implemented.`

In the `**`while` / `until` loops (v18):**` block, the text currently
ends: `Single-line form only (multi-line `if`/loops are a later
iteration); `break N` / `continue N` are not implemented.` Replace that
sentence with:
``break N` / `continue N` are not implemented.`

- [ ] **Step 4: Update the test-suite count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the `passed` counts.
Update the `cargo test               # full test suite (NNN tests)` line
in the Build-and-run section. Expected ≈ 728 — use the actual number.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "v19 task 7: README — multi-line input"
```

---

## Final review checkpoint

After Task 7:

- [ ] `cargo test` shows the expected total passing, 0 failing.
- [ ] `cargo clippy --all-targets -- -D warnings` introduces no new lints
  versus `main` (the codebase has ~38 pre-existing lints; only new ones
  count).
- [ ] Manual REPL smoke session: a multi-line `if`, a multi-line `while`,
  a quote spanning lines, a trailing-`|` pipeline, a backslash-newline
  join, Ctrl-C aborting a partial `if`, and an EOF mid-`if` (piped) → exit
  code 2.
- [ ] Confirm a single-line `if`/`while` still runs exactly as before
  (the retrofit is backward-compatible).
- [ ] Final-review the whole branch as a single diff before merging to
  `main`.
```
