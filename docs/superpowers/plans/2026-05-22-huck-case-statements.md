# huck v21: `case` Statements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the POSIX `case WORD in PATTERN) LIST ;; … esac` statement to huck, with all three terminators (`;;`, `;&`, `;;&`) and the optional leading `(`.

**Architecture:** `case` is a compound command following the v17/v18/v20 pattern (`Command::Case` + `parse_case` + `run_case`). Unlike the earlier loops it also needs lexer work: five new punctuation tokens (`(`, `)`, `;;`, `;&`, `;;&`). Pattern matching reuses the `glob` crate. `run_case` is a small fall-through state machine.

**Tech Stack:** Rust 2024, the `glob` crate (already a dependency).

---

## File Map

| File | Change | Responsibility |
| --- | --- | --- |
| `src/lexer.rs` | Modify | Five new `Operator` tokens |
| `src/command.rs` | Modify | Token plumbing; `Command::Case`, `parse_case`, keywords, `ParseError::UnterminatedCase` |
| `src/continuation.rs` | Modify | Classify `UnterminatedCase`; history joiner learns `case` |
| `src/expand.rs` | Modify | `expand_pattern` — a quote-aware glob-pattern builder |
| `src/executor.rs` | Modify | `run_case`; the `run_command` dispatch arm |
| `src/shell.rs` | Modify | `parse_error_message` arm for `UnterminatedCase` |
| `tests/case_integration.rs` | Create | End-to-end piped `case` scripts |
| `README.md` | Modify | v21 status row and feature note |

**Baseline:** `cargo test` passes 764 tests before this work begins.

---

## Task 1: Lexer tokens and parser plumbing

**Files:**
- Modify: `src/lexer.rs`
- Modify: `src/command.rs`

Adds the five `case` punctuation tokens and teaches the existing parser to handle them: terminators end a sequence/pipeline, stray parens are a syntax error. `case` itself is Task 2. Adding the `Operator` variants makes `parse_pipeline`'s inner redirect `match` non-exhaustive, so the lexer and parser edits land together.

- [ ] **Step 1: Write the failing lexer tests**

In `src/lexer.rs`, inside `#[cfg(test)] mod tests`, add (the `w` helper builds a single-`Literal` `Word` token):

```rust
#[test]
fn tokenize_open_paren() {
    assert_eq!(tokenize("(").unwrap(), vec![Token::Op(Operator::LParen)]);
}

#[test]
fn tokenize_close_paren() {
    assert_eq!(tokenize(")").unwrap(), vec![Token::Op(Operator::RParen)]);
}

#[test]
fn tokenize_double_semi() {
    assert_eq!(tokenize(";;").unwrap(), vec![Token::Op(Operator::DoubleSemi)]);
}

#[test]
fn tokenize_semi_amp() {
    assert_eq!(tokenize(";&").unwrap(), vec![Token::Op(Operator::SemiAmp)]);
}

#[test]
fn tokenize_double_semi_amp() {
    assert_eq!(tokenize(";;&").unwrap(), vec![Token::Op(Operator::DoubleSemiAmp)]);
}

#[test]
fn tokenize_lone_semi_still_semi() {
    assert_eq!(
        tokenize("a;b").unwrap(),
        vec![w("a"), Token::Op(Operator::Semi), w("b")]
    );
}

#[test]
fn tokenize_paren_splits_adjacent_word() {
    assert_eq!(
        tokenize("a)").unwrap(),
        vec![w("a"), Token::Op(Operator::RParen)]
    );
}

#[test]
fn tokenize_quoted_paren_stays_literal() {
    // A quoted `)` is ordinary word content, not an operator.
    assert_eq!(tokenize("')'").unwrap(), vec![w(")")]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib tokenize_open_paren`
Expected: FAIL — compile error: `Operator::LParen` etc. do not exist.

- [ ] **Step 3: Add the five `Operator` variants**

In `src/lexer.rs`, extend the `Operator` enum:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Operator {
    Pipe,           // |
    RedirOut,       // >
    RedirAppend,    // >>
    RedirIn,        // <
    RedirErr,       // 2>
    RedirErrAppend, // 2>>
    And,            // &&
    Or,             // ||
    Semi,           // ;
    Background,     // &
    LParen,         // (
    RParen,         // )
    DoubleSemi,     // ;;
    SemiAmp,        // ;&
    DoubleSemiAmp,  // ;;&
}
```

- [ ] **Step 4: Tokenize `(`, `)`, and the `;`-family**

In `src/lexer.rs`'s `tokenize`, the `';'` arm currently pushes `Operator::Semi` unconditionally. Replace it with a look-ahead scan:

```rust
            ';' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                    in_assignment_value = false;
                }
                let op = if chars.peek() == Some(&';') {
                    chars.next();
                    if chars.peek() == Some(&'&') {
                        chars.next();
                        Operator::DoubleSemiAmp
                    } else {
                        Operator::DoubleSemi
                    }
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    Operator::SemiAmp
                } else {
                    Operator::Semi
                };
                tokens.push(Token::Op(op));
                in_assignment_value = false;
            }
```

Add two new arms to the same `match c`, next to the `;` arm — each flushes any pending word then pushes the token:

```rust
            '(' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                    in_assignment_value = false;
                }
                tokens.push(Token::Op(Operator::LParen));
                in_assignment_value = false;
            }
            ')' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                    in_assignment_value = false;
                }
                tokens.push(Token::Op(Operator::RParen));
                in_assignment_value = false;
            }
```

`(`/`)` inside `'…'`/`"…"` stay literal (the quote scanners never reach this `match`); a `(` immediately after `$` is consumed by the `$` handler before reaching here — both unchanged.

- [ ] **Step 5: Run the lexer tests**

Run: `cargo test --lib tokenize_open_paren tokenize_double_semi`
Expected: still FAIL to compile — `parse_pipeline`'s inner redirect `match op` in `src/command.rs` is now non-exhaustive. Step 6 fixes it.

- [ ] **Step 6: Teach the parser the new tokens**

In `src/command.rs`:

**(a)** `parse` — add a leftover-token guard. It currently ends:

```rust
    let seq = parse_sequence(&mut iter, &[])?;
    Ok(Some(seq))
}
```

Change to:

```rust
    let seq = parse_sequence(&mut iter, &[])?;
    if iter.peek().is_some() {
        // A stray terminator (`;;`/`;&`/`;;&`) left after the top-level
        // sequence — `parse_sequence` peek-breaks on those (see below).
        return Err(ParseError::UnexpectedToken);
    }
    Ok(Some(seq))
}
```

**(b)** `parse_sequence` — its loop-top `match iter.peek()` currently has `None` and `Some(tok)` arms. Add a terminator arm so a `;;`/`;&`/`;;&` ends the sequence (left unconsumed, for `parse_case`):

```rust
        match iter.peek() {
            None => break,
            Some(Token::Op(
                Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
            )) => break,
            Some(tok) => {
                if let Some(kw) = keyword_of(tok) {
                    if stop_at.contains(&kw) {
                        break;
                    }
                }
            }
        }
```

**(c)** `parse_pipeline` — its loop's peek-break `matches!` currently lists `Semi | And | Or | Background`. Add the three terminators so a terminator ends a pipeline cleanly:

```rust
        if matches!(
            token,
            Token::Op(
                Operator::Semi
                    | Operator::And
                    | Operator::Or
                    | Operator::Background
                    | Operator::DoubleSemi
                    | Operator::SemiAmp
                    | Operator::DoubleSemiAmp
            ) | Token::Newline
        ) {
            break;
        }
```

**(d)** `parse_pipeline` — its inner `match token` has arms for `Word`, `Newline`, `Op(Pipe)`, and `Op(op)` (redirects). Add an explicit arm for parens, before the `Op(op)` arm:

```rust
            Token::Op(Operator::LParen | Operator::RParen) => {
                // A `(` or `)` outside a `case` pattern list is a syntax error.
                return Err(ParseError::UnexpectedToken);
            }
```

**(e)** `parse_pipeline` — the inner redirect `match op` ends with an `unreachable!` arm listing `Pipe | And | Or | Semi | Background`. Extend that list to cover the five new variants (they cannot actually reach here — parens have the explicit arm above, terminators are peek-broken — but the match must be exhaustive):

```rust
                    Operator::Pipe
                    | Operator::And
                    | Operator::Or
                    | Operator::Semi
                    | Operator::Background
                    | Operator::LParen
                    | Operator::RParen
                    | Operator::DoubleSemi
                    | Operator::SemiAmp
                    | Operator::DoubleSemiAmp => {
                        unreachable!("handled in the outer arms or peek-break");
                    }
```

- [ ] **Step 7: Write the failing parser tests**

In `src/command.rs`'s `#[cfg(test)] mod tests`, add (helpers `w_tok` exist):

```rust
#[test]
fn stray_close_paren_is_error() {
    assert_eq!(
        parse(vec![w_tok("echo"), Token::Op(Operator::RParen)]),
        Err(ParseError::UnexpectedToken)
    );
}

#[test]
fn stray_open_paren_is_error() {
    assert_eq!(
        parse(vec![w_tok("echo"), Token::Op(Operator::LParen)]),
        Err(ParseError::UnexpectedToken)
    );
}

#[test]
fn stray_double_semi_is_error() {
    assert_eq!(
        parse(vec![w_tok("echo"), Token::Op(Operator::DoubleSemi)]),
        Err(ParseError::UnexpectedToken)
    );
}
```

- [ ] **Step 8: Run the full suite**

Run: `cargo test`
Expected: PASS — the 8 lexer tests and 3 parser tests pass, and all 764 prior tests still pass.

If a *pre-existing* test fails because it fed an unquoted `(` or `)` as ordinary word content, that is the expected ripple (the spec documents it): an unquoted paren is now a metacharacter. Quote the paren in that test (`"("` / `')'`) so it stays literal, and note the change in your report. Do not change shell behaviour.

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs src/command.rs
git commit -m "v21 task 1: case punctuation tokens and parser plumbing"
```

---

## Task 2: AST, keywords, and `parse_case`

**Files:**
- Modify: `src/command.rs`
- Modify: `src/executor.rs` (placeholder `run_command` arm — the real executor is Task 3)
- Modify: `src/shell.rs` (`parse_error_message` arm)
- Modify: `src/continuation.rs` (classifier + history joiner)

- [ ] **Step 1: Write the failing parser tests**

In `src/command.rs`'s `#[cfg(test)] mod tests`, add a `first_case` helper next to `first_for`, and the tests. `w_tok`/`kw`/`plain` exist; `kw` aliases `w_tok`.

```rust
/// Extracts the CaseClause from a sequence whose first command is a Case.
fn first_case(seq: &Sequence) -> &CaseClause {
    match &seq.first {
        Command::Case(c) => c,
        other => panic!("expected a case, got {other:?}"),
    }
}

#[test]
fn parse_simple_case() {
    // case x in a) echo hi ;; esac
    let seq = parse(vec![
        kw("case"), w_tok("x"), kw("in"),
        w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"), w_tok("hi"),
        Token::Op(Operator::DoubleSemi),
        kw("esac"),
    ]).unwrap().unwrap();
    let clause = first_case(&seq);
    assert_eq!(clause.items.len(), 1);
    assert_eq!(clause.items[0].patterns.len(), 1);
    assert_eq!(clause.items[0].terminator, CaseTerminator::Break);
    assert!(clause.items[0].body.is_some());
}

#[test]
fn parse_case_multiline_matches_singleline() {
    let multiline = parse(vec![
        kw("case"), w_tok("x"), kw("in"), Token::Newline,
        w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"), Token::Newline,
        Token::Op(Operator::DoubleSemi), Token::Newline,
        kw("esac"),
    ]).unwrap().unwrap();
    let singleline = parse(vec![
        kw("case"), w_tok("x"), kw("in"),
        w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"),
        Token::Op(Operator::DoubleSemi),
        kw("esac"),
    ]).unwrap().unwrap();
    assert_eq!(multiline, singleline);
}

#[test]
fn parse_case_alternation() {
    // case x in a | b | c) echo hi ;; esac
    let seq = parse(vec![
        kw("case"), w_tok("x"), kw("in"),
        w_tok("a"), Token::Op(Operator::Pipe), w_tok("b"),
        Token::Op(Operator::Pipe), w_tok("c"), Token::Op(Operator::RParen),
        w_tok("echo"), Token::Op(Operator::DoubleSemi),
        kw("esac"),
    ]).unwrap().unwrap();
    assert_eq!(first_case(&seq).items[0].patterns.len(), 3);
}

#[test]
fn parse_case_leading_paren() {
    // case x in (a) echo hi ;; esac
    let seq = parse(vec![
        kw("case"), w_tok("x"), kw("in"),
        Token::Op(Operator::LParen), w_tok("a"), Token::Op(Operator::RParen),
        w_tok("echo"), Token::Op(Operator::DoubleSemi),
        kw("esac"),
    ]).unwrap().unwrap();
    assert_eq!(first_case(&seq).items[0].patterns.len(), 1);
}

#[test]
fn parse_case_empty_body() {
    // case x in a) ;; esac  — empty clause body
    let seq = parse(vec![
        kw("case"), w_tok("x"), kw("in"),
        w_tok("a"), Token::Op(Operator::RParen),
        Token::Op(Operator::DoubleSemi),
        kw("esac"),
    ]).unwrap().unwrap();
    assert!(first_case(&seq).items[0].body.is_none());
}

#[test]
fn parse_case_terminators() {
    // three clauses, one per terminator
    let seq = parse(vec![
        kw("case"), w_tok("x"), kw("in"),
        w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"),
        Token::Op(Operator::DoubleSemi),
        w_tok("b"), Token::Op(Operator::RParen), w_tok("echo"),
        Token::Op(Operator::SemiAmp),
        w_tok("c"), Token::Op(Operator::RParen), w_tok("echo"),
        Token::Op(Operator::DoubleSemiAmp),
        kw("esac"),
    ]).unwrap().unwrap();
    let items = &first_case(&seq).items;
    assert_eq!(items[0].terminator, CaseTerminator::Break);
    assert_eq!(items[1].terminator, CaseTerminator::FallThrough);
    assert_eq!(items[2].terminator, CaseTerminator::ContinueMatch);
}

#[test]
fn parse_case_omitted_final_terminator() {
    // case x in a) echo hi esac  — no `;;` before `esac`
    let seq = parse(vec![
        kw("case"), w_tok("x"), kw("in"),
        w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"),
        kw("esac"),
    ]).unwrap().unwrap();
    assert_eq!(first_case(&seq).items[0].terminator, CaseTerminator::Break);
}

#[test]
fn parse_case_empty() {
    // case x in esac  — no clauses
    let seq = parse(vec![kw("case"), w_tok("x"), kw("in"), kw("esac")])
        .unwrap()
        .unwrap();
    assert!(first_case(&seq).items.is_empty());
}

#[test]
fn parse_case_unterminated_is_unterminated_case() {
    assert_eq!(parse(vec![kw("case")]), Err(ParseError::UnterminatedCase));
    assert_eq!(
        parse(vec![kw("case"), w_tok("x")]),
        Err(ParseError::UnterminatedCase)
    );
    assert_eq!(
        parse(vec![kw("case"), w_tok("x"), kw("in")]),
        Err(ParseError::UnterminatedCase)
    );
    // a clause with no `esac`
    assert_eq!(
        parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"),
            Token::Op(Operator::DoubleSemi),
        ]),
        Err(ParseError::UnterminatedCase)
    );
}

#[test]
fn parse_case_malformed_pattern_list_errors() {
    // case x in a b) ...  — two pattern words with no `|`
    assert_eq!(
        parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            w_tok("a"), w_tok("b"), Token::Op(Operator::RParen),
            w_tok("echo"), Token::Op(Operator::DoubleSemi),
            kw("esac"),
        ]),
        Err(ParseError::UnexpectedToken)
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib parse_simple_case`
Expected: FAIL — compile error: `Command::Case`, `CaseClause`, `CaseTerminator`, `ParseError::UnterminatedCase`, `first_case` do not exist.

- [ ] **Step 3: Add the keywords**

In `src/command.rs`, extend `Keyword` (after `In`):

```rust
enum Keyword {
    If, Then, Elif, Else, Fi,
    While, Until, Do, Done,
    For, In,
    Case, Esac,
}
```

(Keep the existing one-per-line layout; the compressed form above just shows the additions.) Add to `Keyword::name`:

```rust
            Keyword::Case => "case",
            Keyword::Esac => "esac",
```

Add to `keyword_of`:

```rust
        "case" => Some(Keyword::Case),
        "esac" => Some(Keyword::Esac),
```

- [ ] **Step 4: Add the AST**

In `src/command.rs`, extend `Command`:

```rust
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
    While(Box<WhileClause>),
    For(Box<ForClause>),
    Case(Box<CaseClause>),
}
```

Add the structs after `ForClause`:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CaseClause {
    /// The word being matched — unexpanded.
    pub subject: Word,
    /// The clauses, in source order. May be empty.
    pub items: Vec<CaseItem>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CaseItem {
    /// The `|`-separated patterns, unexpanded. Always non-empty.
    pub patterns: Vec<Word>,
    /// The clause body. `None` means an empty body.
    pub body: Option<Sequence>,
    pub terminator: CaseTerminator,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CaseTerminator {
    Break,         // ;;
    FallThrough,   // ;&
    ContinueMatch, // ;;&
}
```

Add the `UnterminatedCase` variant to `ParseError`:

```rust
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
    ForVariable,
    UnterminatedCase,
}
```

- [ ] **Step 5: Add `parse_case` and `parse_case_item`**

In `src/command.rs`, add next to `parse_for`:

```rust
/// Parses `case WORD in [clause]... esac`. The caller has peeked `case`.
fn parse_case<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<CaseClause, ParseError> {
    expect_keyword(iter, Keyword::Case, ParseError::UnterminatedCase)?;
    skip_newlines(iter);

    let subject = match iter.next() {
        None => return Err(ParseError::UnterminatedCase),
        Some(Token::Word(w)) => w,
        Some(_) => return Err(ParseError::UnexpectedToken),
    };

    skip_newlines(iter);
    expect_keyword(iter, Keyword::In, ParseError::UnterminatedCase)?;
    skip_newlines(iter);

    let mut items: Vec<CaseItem> = Vec::new();
    while iter.peek().and_then(keyword_of) != Some(Keyword::Esac) {
        if iter.peek().is_none() {
            return Err(ParseError::UnterminatedCase);
        }
        items.push(parse_case_item(iter)?);
        skip_newlines(iter);
    }
    expect_keyword(iter, Keyword::Esac, ParseError::UnterminatedCase)?;
    Ok(CaseClause { subject, items })
}

/// Parses one `[(] pattern [| pattern]... ) [body] [terminator]` clause.
fn parse_case_item<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<CaseItem, ParseError> {
    // Optional leading `(`.
    if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
        iter.next();
    }

    // Pattern list — Word (`|` Word)* `)`, non-empty.
    let mut patterns: Vec<Word> = Vec::new();
    loop {
        skip_newlines(iter);
        match iter.next() {
            None => return Err(ParseError::UnterminatedCase),
            Some(Token::Word(w)) => patterns.push(w),
            Some(_) => return Err(ParseError::UnexpectedToken),
        }
        match iter.peek() {
            None => return Err(ParseError::UnterminatedCase),
            Some(Token::Op(Operator::Pipe)) => {
                iter.next();
            }
            Some(Token::Op(Operator::RParen)) => {
                iter.next();
                break;
            }
            Some(_) => return Err(ParseError::UnexpectedToken),
        }
    }

    // Body — empty if the next token is a terminator or `esac`.
    skip_newlines(iter);
    let body = match iter.peek() {
        None => return Err(ParseError::UnterminatedCase),
        Some(Token::Op(
            Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
        )) => None,
        Some(tok) if keyword_of(tok) == Some(Keyword::Esac) => None,
        Some(_) => Some(parse_sequence(iter, &[Keyword::Esac])?),
    };

    // Terminator — an absent one (next token is `esac` or end) is `Break`.
    let terminator = match iter.peek() {
        Some(Token::Op(Operator::DoubleSemi)) => {
            iter.next();
            CaseTerminator::Break
        }
        Some(Token::Op(Operator::SemiAmp)) => {
            iter.next();
            CaseTerminator::FallThrough
        }
        Some(Token::Op(Operator::DoubleSemiAmp)) => {
            iter.next();
            CaseTerminator::ContinueMatch
        }
        _ => CaseTerminator::Break,
    };

    Ok(CaseItem { patterns, body, terminator })
}
```

- [ ] **Step 6: Wire `parse_case` into `parse_command`**

In `src/command.rs`, `parse_command`'s `match` has arms for `If`, `While`/`Until`, `For`, `other`, `None`. Add a `Case` arm:

```rust
        Some(Keyword::For) => Ok(Command::For(Box::new(parse_for(iter)?))),
        Some(Keyword::Case) => Ok(Command::Case(Box::new(parse_case(iter)?))),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
```

- [ ] **Step 7: Add the test-helper arms for the new `Command` variant**

In `src/command.rs`'s test module, `first_pipeline` and `first_if` have exhaustive `match`es on `&seq.first`. Add a `Case` arm to each:

```rust
            Command::For(_) => panic!("expected a pipeline, got a for"),
            Command::Case(_) => panic!("expected a pipeline, got a case"),
```

```rust
            Command::For(_) => panic!("expected an if, got a for"),
            Command::Case(_) => panic!("expected an if, got a case"),
```

- [ ] **Step 8: Add the placeholder executor arm**

In `src/executor.rs`, `run_command`'s `match` gains a placeholder `Case` arm (the real `run_case` is Task 3):

```rust
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink),
        Command::If(clause) => run_if(clause, shell, sink),
        Command::While(clause) => run_while(clause, shell, sink),
        Command::For(clause) => run_for(clause, shell, sink),
        Command::Case(_) => unreachable!("run_case lands in v21 task 3"),
    }
}
```

- [ ] **Step 9: Add the `UnterminatedCase` message in `shell.rs`**

In `src/shell.rs`, `parse_error_message`'s `match` over `ParseError` gains an arm:

```rust
        ParseError::UnterminatedCase => "unterminated 'case' (expected 'esac')".to_string(),
```

- [ ] **Step 10: Teach the continuation classifier about `case`**

In `src/continuation.rs`:

`classify`'s parse-result `match` currently maps `Err(ParseError::UnterminatedIf | ParseError::UnterminatedLoop)` to `Incomplete(Compound)`. Add `UnterminatedCase`:

```rust
        Err(ParseError::UnterminatedIf | ParseError::UnterminatedLoop | ParseError::UnterminatedCase) => {
            Completeness::Incomplete(ContinuationReason::Compound)
        }
```

`ends_with_control_keyword`'s `matches!` gains `"case"`:

```rust
        Some("if" | "while" | "until" | "then" | "do" | "else" | "elif" | "for" | "in" | "case")
```

Add two tests to `src/continuation.rs`'s `#[cfg(test)] mod tests`:

```rust
#[test]
fn unterminated_case_is_incomplete() {
    assert_eq!(
        classify("case x in a) echo hi"),
        Completeness::Incomplete(ContinuationReason::Compound)
    );
}

#[test]
fn joiner_compound_is_space_after_case_keyword() {
    assert_eq!(joiner_for(ContinuationReason::Compound, "case"), " ");
}
```

- [ ] **Step 11: Run the full suite**

Run: `cargo test`
Expected: PASS — the 10 new parser tests, the 2 continuation tests, and all prior tests pass.

- [ ] **Step 12: Commit**

```bash
git add src/command.rs src/executor.rs src/shell.rs src/continuation.rs
git commit -m "v21 task 2: case AST, keywords, and parser"
```

---

## Task 3: Executor — `run_case`

**Files:**
- Modify: `src/expand.rs` (new `expand_pattern` helper)
- Modify: `src/executor.rs` (`run_case`)

- [ ] **Step 1: Add `expand_pattern` to `expand.rs`**

`src/expand.rs` has `expand_assignment(word, shell) -> String`, which expands a `Word` to a single string with no field splitting. `expand_pattern` produces a glob-pattern string the same way, but escapes the parts that were quoted in the source so a quoted `*`/`?`/`[` matches literally.

Add to `src/expand.rs`:

```rust
/// True when `part` carried a `quoted` flag set to true. Tilde parts
/// have no quoted flag and count as unquoted.
fn word_part_is_quoted(part: &WordPart) -> bool {
    match part {
        WordPart::Literal { quoted, .. } => *quoted,
        WordPart::Var { quoted, .. } => *quoted,
        WordPart::LastStatus { quoted } => *quoted,
        WordPart::CommandSub { quoted, .. } => *quoted,
        WordPart::Arith { quoted, .. } => *quoted,
        WordPart::ParamExpansion { quoted, .. } => *quoted,
        WordPart::Tilde(_) => false,
    }
}

/// Expands `word` into a glob-pattern string for `case` matching.
/// Like `expand_assignment` (no field splitting), but text contributed
/// by a quoted part is escaped via `glob::Pattern::escape`, so a quoted
/// `*`/`?`/`[` matches literally while an unquoted one is a wildcard.
pub fn expand_pattern(word: &Word, shell: &mut Shell) -> String {
    let mut result = String::new();
    for part in &word.0 {
        let text = expand_assignment(&Word(vec![part.clone()]), shell);
        if word_part_is_quoted(part) {
            result.push_str(&glob::Pattern::escape(&text));
        } else {
            result.push_str(&text);
        }
    }
    result
}
```

(`expand_assignment` of a one-part `Word` returns exactly that part's expanded text; concatenating per part yields the same string as expanding the whole word, with quoted parts now escaped. `WordPart` derives `Clone`.)

- [ ] **Step 2: Write the failing executor tests**

`src/executor.rs`'s `#[cfg(test)] mod tests` has helpers `echo_seq`, `lit_word`, `execute_capturing`, `break_seq`. Add a `use` and helpers:

```rust
use crate::command::{CaseClause, CaseItem, CaseTerminator};

/// A Sequence wrapping a single `case` clause.
fn case_seq(clause: CaseClause) -> Sequence {
    Sequence { first: Command::Case(Box::new(clause)), rest: vec![], background: false }
}

/// A CaseItem with a `;;` terminator.
fn item(patterns: &[&str], body: Option<Sequence>) -> CaseItem {
    CaseItem {
        patterns: patterns.iter().map(|p| lit_word(p)).collect(),
        body,
        terminator: CaseTerminator::Break,
    }
}
```

Then the tests:

```rust
#[test]
fn case_runs_first_matching_clause() {
    let clause = CaseClause {
        subject: lit_word("foo"),
        items: vec![
            item(&["foo"], Some(echo_seq("matched"))),
            item(&["bar"], Some(echo_seq("other"))),
        ],
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "matched");
    assert_eq!(status, 0);
}

#[test]
fn case_glob_pattern_matches() {
    let clause = CaseClause {
        subject: lit_word("report.txt"),
        items: vec![item(&["*.txt"], Some(echo_seq("text")))],
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "text");
}

#[test]
fn case_alternation_matches_any() {
    let clause = CaseClause {
        subject: lit_word("b"),
        items: vec![item(&["a", "b", "c"], Some(echo_seq("hit")))],
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "hit");
}

#[test]
fn case_no_match_is_status_zero_no_output() {
    let clause = CaseClause {
        subject: lit_word("x"),
        items: vec![item(&["y"], Some(echo_seq("nope")))],
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn case_empty_body_is_status_zero() {
    let clause = CaseClause {
        subject: lit_word("x"),
        items: vec![item(&["x"], None)],
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn case_fall_through_runs_next_body() {
    // a) echo one ;&  *) echo two ;;
    let clause = CaseClause {
        subject: lit_word("a"),
        items: vec![
            CaseItem {
                patterns: vec![lit_word("a")],
                body: Some(echo_seq("one")),
                terminator: CaseTerminator::FallThrough,
            },
            item(&["*"], Some(echo_seq("two"))),
        ],
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["one", "two"]);
}

#[test]
fn case_continue_match_keeps_testing() {
    // a) echo one ;;&  a) echo two ;;
    let clause = CaseClause {
        subject: lit_word("a"),
        items: vec![
            CaseItem {
                patterns: vec![lit_word("a")],
                body: Some(echo_seq("one")),
                terminator: CaseTerminator::ContinueMatch,
            },
            item(&["a"], Some(echo_seq("two"))),
        ],
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&case_seq(clause), &mut shell);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["one", "two"]);
}

#[test]
fn case_quoted_metacharacter_matches_literally() {
    // pattern is a quoted `*` — matches the literal string "*", not "abc"
    let star_pattern = Word(vec![WordPart::Literal { text: "*".to_string(), quoted: true }]);
    let make = |subj: &str| CaseClause {
        subject: lit_word(subj),
        items: vec![CaseItem {
            patterns: vec![star_pattern.clone()],
            body: Some(echo_seq("hit")),
            terminator: CaseTerminator::Break,
        }],
    };
    let mut shell = Shell::new();
    let (out_star, _) = execute_capturing(&case_seq(make("*")), &mut shell);
    assert_eq!(out_star.trim(), "hit", "literal * should match the string \"*\"");
    let (out_abc, _) = execute_capturing(&case_seq(make("abc")), &mut shell);
    assert_eq!(out_abc.trim(), "", "quoted * must not act as a wildcard");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib case_runs_first_matching_clause`
Expected: FAIL — `run_command`'s `Command::Case` arm is the `unreachable!` placeholder, so it panics.

- [ ] **Step 4: Implement `run_case` and wire it in**

In `src/executor.rs`, add `expand_pattern` to the `use crate::expand::{…}` import, and add `CaseClause`/`CaseItem`/`CaseTerminator` to the `use crate::command::{…}` import.

Replace the `run_command` placeholder arm:

```rust
        Command::Case(clause) => run_case(clause, shell, sink),
```

Add `run_case` and `case_item_matches` next to `run_for`:

```rust
/// Matches `subject` against a `case` clause's `|`-patterns. A clause
/// matches if any pattern matches; an unparseable glob matches nothing.
fn case_item_matches(item: &CaseItem, subject: &str, shell: &mut Shell) -> bool {
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    for pattern_word in &item.patterns {
        let pattern = expand_pattern(pattern_word, shell);
        if let Ok(p) = glob::Pattern::new(&pattern) {
            if p.matches_with(subject, opts) {
                return true;
            }
        }
    }
    false
}

/// Runs a `case` statement. The subject is expanded once; clauses are
/// walked in order. The first matching clause's body runs, then the
/// terminator decides what happens: `;;` stops, `;&` runs the next
/// clause's body unconditionally, `;;&` resumes pattern-testing.
/// `case` is not a loop — `break`/`continue` propagate out unchanged.
fn run_case(clause: &CaseClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let subject = expand_assignment(&clause.subject, shell);
    let mut last = ExecOutcome::Continue(0);
    let mut i = 0;
    let mut fall_through = false;
    while i < clause.items.len() {
        let item = &clause.items[i];
        let run_this = fall_through || case_item_matches(item, &subject, shell);
        if !run_this {
            i += 1;
            continue;
        }
        match &item.body {
            None => last = ExecOutcome::Continue(0),
            Some(body) => match execute_sequence_body(body, shell, sink) {
                ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
                ExecOutcome::LoopBreak => return ExecOutcome::LoopBreak,
                ExecOutcome::LoopContinue => return ExecOutcome::LoopContinue,
                ExecOutcome::Continue(c) => last = ExecOutcome::Continue(c),
            },
        }
        match item.terminator {
            CaseTerminator::Break => return last,
            CaseTerminator::FallThrough => {
                fall_through = true;
                i += 1;
            }
            CaseTerminator::ContinueMatch => {
                fall_through = false;
                i += 1;
            }
        }
    }
    last
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test`
Expected: PASS — the 8 new executor tests plus all prior tests.

- [ ] **Step 6: Manual smoke test**

```bash
cargo build --release
./target/release/huck <<'EOF'
case hello in
  hi) echo greeting ;;
  hello) echo hello-world ;;
  *) echo unknown ;;
esac
case report.txt in *.txt) echo is-text ;; *) echo not-text ;; esac
case b in a|b|c) echo in-list ;; esac
for x in a b c; do case $x in b) echo got-b ;; *) echo got-$x ;; esac; done
case a in a) echo first ;& fallen) echo through ;; esac
case x in y) echo no ;; esac
echo done-$?
exit
EOF
```

Expected output, in order: `hello-world`; `is-text`; `in-list`; `got-a`, `got-b`, `got-c`; `first`, `through`; `done-0`. Report the actual output verbatim.

- [ ] **Step 7: Commit**

```bash
git add src/expand.rs src/executor.rs
git commit -m "v21 task 3: executor run_case — case-statement execution"
```

---

## Task 4: End-to-end integration tests

**Files:**
- Create: `tests/case_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/case_integration.rs`:

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

#[test]
fn case_basic_match() {
    let (out, _) = run("case hello in hi) echo a;; hello) echo b;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "b"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "a"), "stdout: {out}");
}

#[test]
fn case_glob_pattern() {
    let (out, _) = run("case report.txt in *.txt) echo text;; *) echo other;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "text"), "stdout: {out}");
}

#[test]
fn case_question_mark_pattern() {
    let (out, _) = run("case ab in ??) echo two;; ?) echo one;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "two"), "stdout: {out}");
}

#[test]
fn case_alternation() {
    let (out, _) = run("case b in a|b|c) echo in-list;; *) echo no;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "in-list"), "stdout: {out}");
}

#[test]
fn case_catch_all_star() {
    let (out, _) = run("case zzz in a) echo a;; *) echo fallback;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "fallback"), "stdout: {out}");
}

#[test]
fn case_no_match_runs_nothing() {
    let (out, _) = run("case x in y) echo no;; esac\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "no"), "stdout: {out}");
}

#[test]
fn case_multiline() {
    let script = "case dog in\n  cat) echo meow ;;\n  dog) echo woof ;;\nesac\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "woof"), "stdout: {out}");
}

#[test]
fn case_subject_is_variable() {
    let (out, _) = run("x=apple\ncase $x in apple) echo fruit;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "fruit"), "stdout: {out}");
}

#[test]
fn case_fall_through() {
    let (out, _) = run("case a in a) echo one ;& b) echo two ;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "one"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "two"), "stdout: {out}");
}

#[test]
fn case_continue_match() {
    // both `a*` and `*b` match "ab"; ;;& keeps testing
    let (out, _) = run("case ab in a*) echo first ;;& *b) echo second ;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "first"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "second"), "stdout: {out}");
}

#[test]
fn case_leading_paren_form() {
    let (out, _) = run("case a in (a) echo paren;; esac\nexit\n");
    assert!(out.lines().any(|l| l == "paren"), "stdout: {out}");
}

#[test]
fn case_quoted_metacharacter_is_literal() {
    // the pattern "*" (quoted) matches only the literal string *, not abc
    let (out1, _) = run("case abc in \"*\") echo wild;; *) echo other;; esac\nexit\n");
    assert!(out1.lines().any(|l| l == "other"), "quoted * must not match abc: {out1}");
    let (out2, _) = run("case * in \"*\") echo literal;; esac\nexit\n");
    assert!(out2.lines().any(|l| l == "literal"), "quoted * should match \"*\": {out2}");
}

#[test]
fn case_nested_in_for() {
    let (out, _) = run(
        "for x in a b c; do case $x in b) echo got-b;; *) echo skip-$x;; esac; done\nexit\n",
    );
    assert!(out.lines().any(|l| l == "got-b"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "skip-a"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "skip-c"), "stdout: {out}");
}

#[test]
fn break_from_case_inside_loop() {
    // `break` inside a case body targets the enclosing while loop
    let script = "i=0\nwhile test $i -lt 5; do i=$((i+1)); case $i in 3) break;; *) echo n$i;; esac; done\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "n1"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "n2"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "n3"), "loop should have broken: {out}");
    assert!(!out.lines().any(|l| l == "n4"), "loop should have broken: {out}");
}

#[test]
fn case_empty_body() {
    let (out, _) = run("case x in x) ;; *) echo other;; esac\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "other"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test case_integration`
Expected: 15 tests pass.

If a test fails, inspect the actual stdout/stderr. `case` behaviour was implemented in Tasks 1-3; only adjust an assertion if it genuinely mismatches *correct* shell output, and explain why. NEVER change `src/` to make a test pass; if a failure looks like a real bug, report DONE_WITH_CONCERNS or BLOCKED.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: PASS — all tests, prior plus the 15 new.

- [ ] **Step 4: Commit**

```bash
git add tests/case_integration.rs
git commit -m "v21 task 4: end-to-end case-statement integration tests"
```

---

## Task 5: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v21 row to the status table**

Append after the v20 row, matching the table's column alignment:

```
| v21       | `case` statements (`case W in PAT) … ;; esac`)          |
```

- [ ] **Step 2: Add a feature note**

After the v20 `for`-loops block, add:

```markdown
**`case` statements (v21):**
`case WORD in PATTERN) LIST ;; … esac` matches the expanded subject
against each clause's glob patterns (`*`, `?`, `[…]`), runs the first
matching clause's body, and stops. Patterns may be `|`-alternated and
may carry an optional leading `(`. A quoted metacharacter matches
literally (`"*"` matches a literal `*`). All three terminators are
supported: `;;` (done), `;&` (fall through into the next clause's
body), `;;&` (keep testing later patterns). Clause bodies may be empty
and the final `;;` may be omitted. `break`/`continue` inside a body
target the enclosing loop — `case` is not a loop. Multi-line form
works as for the other compound commands.
```

- [ ] **Step 3: Update the "Not yet implemented" section**

The README's "Not yet implemented:" paragraph lists control flow. `case` is part of `control flow (case)` (or similar — read the actual text). Remove `case`: if it reads `control flow (case), functions, here-docs, aliases.` it becomes `functions, here-docs, aliases.` — read the real wording and edit precisely. If `case` is not mentioned there, note that in your report.

- [ ] **Step 4: Update the test-suite count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the `passed` counts across all lines. Update the `cargo test               # full test suite (NNN tests)` line in the Build-and-run section. Expected ≈ 810 — use the real summed number.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "v21 task 5: README — case statements"
```

---

## Final review checkpoint

After Task 5:

- [ ] `cargo test` shows the expected total passing, 0 failing.
- [ ] `cargo clippy --all-targets -- -D warnings` introduces no new lints versus `main` (the codebase has ~38 pre-existing lints; only new ones count).
- [ ] Manual REPL smoke session: a basic `case`, glob and `?` patterns, `|`-alternation, the `*` catch-all, a multi-line `case`, `;&` and `;;&`, the `(pattern)` form, a quoted-metacharacter literal match, an empty body, a `case` nested in a `for`, `break` from a `case` inside a `while`, and a no-match `case` leaving `$?` at 0.
- [ ] Confirm `if`/`while`/`until`/`for` and single-line behaviour still work; confirm a quoted `(`/`)` is still a literal word.
- [ ] Final-review the whole branch as a single diff before merging to `main`.
```
