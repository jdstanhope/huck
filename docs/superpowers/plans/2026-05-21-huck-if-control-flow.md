# huck v17: `if` Control Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `if`/`then`/`elif`/`else`/`fi` conditional — huck's first compound command.

**Architecture:** The flat AST (`Sequence` of `Pipeline`) becomes `Sequence` of `Command`, where `Command` is `Pipeline` or `If`. The parser is rewritten as keyword-aware recursive descent. The executor's sequence-runner dispatches each `Command`, with a new `run_if`. No lexer change — `if`/`then`/`elif`/`else`/`fi` are recognized positionally by the parser.

**Tech Stack:** Rust 2024 edition. No new dependencies.

**Reference:** Design spec at `docs/superpowers/specs/2026-05-21-huck-if-control-flow-design.md`.

**Limitations carried by this plan (beyond the spec's stated scope):** `&` (backgrounding) inside an `if` condition or body is rejected as a parse error — `&` is permitted only at the top level. Backgrounding a whole `if` (`if ...; fi &`) parses but runs synchronously (the spec documents this).

---

## File Map

- **Modify:** `src/command.rs` — `Command`/`IfClause`/`ElifBranch`, `Sequence` restructure, recursive-descent parser, new `ParseError` variants
- **Modify:** `src/executor.rs` — `Command` dispatch, `run_if`
- **Modify:** `src/shell.rs` — `ParseError` display for the new variants
- **New:** `tests/if_integration.rs`
- **Modify:** `README.md` — v17 row, features note, test count

---

## Task 1: AST restructuring

Introduce `Command`, `IfClause`, `ElifBranch`; change `Sequence` to hold `Command`s. Update the existing parser to wrap every pipeline in `Command::Pipeline`, and the executor to dispatch. No `if` behavior yet — everything stays green as all-`Command::Pipeline`.

**Files:**
- Modify: `src/command.rs`
- Modify: `src/executor.rs`

- [ ] **Step 1: Add the new AST types and restructure `Sequence`**

In `src/command.rs`, replace the `Sequence` struct definition and add the new types. The current `Sequence` is:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Sequence {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
    pub background: bool,
}
```

Replace it with:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IfClause {
    pub condition: Sequence,
    pub then_body: Sequence,
    pub elif_branches: Vec<ElifBranch>,
    pub else_body: Option<Sequence>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ElifBranch {
    pub condition: Sequence,
    pub body: Sequence,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Sequence {
    pub first: Command,
    pub rest: Vec<(Connector, Command)>,
    pub background: bool,
}
```

- [ ] **Step 2: Wrap pipelines in the existing `parse`**

In `src/command.rs`'s `parse` function, the existing code builds `Sequence { first: <Pipeline>, rest: Vec<(Connector, Pipeline)>, .. }`. Change it minimally so `first` and each `rest` element wrap the pipeline:

- `let first = parse_pipeline(&mut iter)?;` → `let first = Command::Pipeline(parse_pipeline(&mut iter)?);`
- Each `rest.push((Connector::Semi, pipeline));` (and `And`, `Or`) → `rest.push((Connector::Semi, Command::Pipeline(pipeline)));`

`rest`'s declared type is now `Vec<(Connector, Command)>`. Do **not** restructure the parser yet — that is Task 2. This is the minimal wrap so it compiles and behaves identically.

- [ ] **Step 3: Update the executor to dispatch `Command`**

In `src/executor.rs`:

Add `Command` and `IfClause` to the `use crate::command::{...}` import.

In `execute`, the background branch currently does
`return run_background_sequence(&seq.first, shell, &mut sink, source);`.
`seq.first` is now a `Command`. `run_background_sequence` stays
pipeline-only. Change `execute`:

```rust
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    if seq.background {
        if let Command::Pipeline(p) = &seq.first {
            // Parser guarantees rest.is_empty() when background is set.
            return run_background_sequence(p, shell, &mut sink, source);
        }
        // Backgrounding a compound command (if) is not supported in v17;
        // fall through and run it synchronously.
    }
    execute_sequence_body(seq, shell, &mut sink)
}
```

In `execute_sequence_body`, the body runs `seq.first` and iterates `seq.rest`. Change it to dispatch through a new `run_command`:

```rust
fn execute_sequence_body(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let mut status = run_command(&seq.first, shell, sink);
    if matches!(status, ExecOutcome::Exit(_)) {
        return status;
    }
    for (connector, command) in &seq.rest {
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_command(command, shell, sink);
            if matches!(status, ExecOutcome::Exit(_)) {
                return status;
            }
        }
    }
    status
}

/// Dispatches a single sequence element.
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink),
        Command::If(_) => unreachable!("if execution lands in v17 task 3"),
    }
}
```

`run_pipeline`, `run_background_sequence`, and everything below are unchanged.

- [ ] **Step 4: Fix the test helpers in `command.rs`**

`src/command.rs`'s `#[cfg(test)] mod tests` has helpers that build/inspect `Sequence`. Update them:

```rust
fn one_pipeline(commands: Vec<SimpleCommand>) -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline { commands }),
        rest: vec![],
        background: false,
    }
}

/// Reaches through `Command::Pipeline` for tests that inspect the first
/// element as a pipeline.
fn first_pipeline(seq: &Sequence) -> &Pipeline {
    match &seq.first {
        Command::Pipeline(p) => p,
        Command::If(_) => panic!("expected a pipeline, got an if"),
    }
}
```

Rewrite `exec_stdout`, `exec_stdin`, `exec_stderr` to go through `first_pipeline`:

```rust
fn exec_stdout(seq: &Sequence) -> &Option<Redirect> {
    match &first_pipeline(seq).commands[0] {
        SimpleCommand::Exec(e) => &e.stdout,
        _ => panic!("expected Exec"),
    }
}
```

(and the analogous `exec_stdin` / `exec_stderr`.)

- [ ] **Step 5: Fix the remaining `command.rs` test compile errors**

Run: `cargo build --tests 2>&1 | head -40`

Every remaining error is the same shape: a test accesses `seq.first.commands` or `seq.rest[i].1` expecting a `Pipeline`. Fix mechanically:
- `seq.first.commands` → `first_pipeline(&seq).commands`
- `seq.rest[0].1` (if any test inspects it as a pipeline) → match `Command::Pipeline`
- `seq.rest[0].0` (the `Connector`) is unchanged.
- Tests that build a `Sequence` literally → wrap the pipeline in `Command::Pipeline`.
- The `parse_assignment_with_command_sub_value_moves_parts` test builds an `inner_seq` `Sequence` literal — wrap its `first` in `Command::Pipeline`.

- [ ] **Step 6: Fix the `executor.rs` test helpers**

`src/executor.rs`'s `#[cfg(test)] mod tests` has `one_command_sequence` and literal `Sequence`s. Update them to wrap pipelines:

```rust
fn one_command_sequence(cmd: SimpleCommand) -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline { commands: vec![cmd] }),
        rest: vec![],
        background: false,
    }
}
```

For the literal `Sequence { first: Pipeline { ... }, .. }` constructions in executor tests, wrap each `first` in `Command::Pipeline(...)`. Run `cargo build --tests` and fix any remaining errors mechanically.

- [ ] **Step 7: Run the full suite**

Run: `cargo test`
Expected: all tests pass (620 baseline, same count — no new tests, behavior unchanged). `Command::If` and the new types carry dead-code warnings (no producer yet) — expected.

- [ ] **Step 8: Commit**

```bash
git add src/command.rs src/executor.rs
git commit -m "v17 task 1: restructure AST — Sequence of Command (Pipeline or If)"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/if-control-flow`
- Baseline: 620 tests passing
- This task is purely a structural change — no `if` behavior. Every `Sequence` still has a `Command::Pipeline` as its `first` and in every `rest` element.
- `expand.rs` carries a `Sequence` in `WordPart::CommandSub` and passes it to the executor; it does not inspect `.first`, so it needs no change beyond recompiling. If `expand.rs` *tests* build a `Sequence` literal, wrap as above.

## Self-Review

- Does `cargo test` pass with the same 620 count?
- Did you update both `command.rs` and `executor.rs` test helpers?
- Is `run_command`'s `If` arm an `unreachable!` (no producer yet)?

## Report Format

Status, files changed, test count, commit SHA, any concerns.

---

## Task 2: Keyword-aware recursive-descent parser

Rewrite the parse layer in `command.rs` as recursive descent that recognizes `if`/`then`/`elif`/`else`/`fi` and produces `Command::If`. The executor still `unreachable!`s on `Command::If`, so this task's tests are parser-only (they assert the AST, never execute).

**Files:**
- Modify: `src/command.rs`

- [ ] **Step 1: Write failing parser tests**

Add to `src/command.rs` `#[cfg(test)] mod tests`. First, helpers:

```rust
/// A bare unquoted keyword/word token.
fn kw(s: &str) -> Token {
    w_tok(s)
}

/// Extracts the IfClause from a sequence whose first command is an If.
fn first_if(seq: &Sequence) -> &IfClause {
    match &seq.first {
        Command::If(c) => c,
        Command::Pipeline(_) => panic!("expected an if, got a pipeline"),
    }
}
```

Then the tests:

```rust
#[test]
fn parse_simple_if() {
    // if a; then b; fi
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi),
        kw("fi"),
    ]).unwrap().unwrap();
    let c = first_if(&seq);
    assert_eq!(c.condition.first, Command::Pipeline(Pipeline { commands: vec![plain("a", &[])] }));
    assert_eq!(c.then_body.first, Command::Pipeline(Pipeline { commands: vec![plain("b", &[])] }));
    assert!(c.elif_branches.is_empty());
    assert!(c.else_body.is_none());
}

#[test]
fn parse_if_else() {
    // if a; then b; else c; fi
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi),
        kw("else"), w_tok("c"), Token::Op(Operator::Semi),
        kw("fi"),
    ]).unwrap().unwrap();
    let c = first_if(&seq);
    assert!(c.else_body.is_some());
}

#[test]
fn parse_if_elif_else() {
    // if a; then b; elif c; then d; else e; fi
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi),
        kw("elif"), w_tok("c"), Token::Op(Operator::Semi),
        kw("then"), w_tok("d"), Token::Op(Operator::Semi),
        kw("else"), w_tok("e"), Token::Op(Operator::Semi),
        kw("fi"),
    ]).unwrap().unwrap();
    let c = first_if(&seq);
    assert_eq!(c.elif_branches.len(), 1);
    assert!(c.else_body.is_some());
}

#[test]
fn parse_if_with_andor_condition() {
    // if a && b; then c; fi
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::And), w_tok("b"),
        Token::Op(Operator::Semi),
        kw("then"), w_tok("c"), Token::Op(Operator::Semi),
        kw("fi"),
    ]).unwrap().unwrap();
    let c = first_if(&seq);
    assert_eq!(c.condition.rest.len(), 1);
    assert_eq!(c.condition.rest[0].0, Connector::And);
}

#[test]
fn parse_if_multi_command_body() {
    // if a; then b; c; fi
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi), w_tok("c"),
        Token::Op(Operator::Semi),
        kw("fi"),
    ]).unwrap().unwrap();
    let c = first_if(&seq);
    assert_eq!(c.then_body.rest.len(), 1);
}

#[test]
fn parse_if_followed_by_command() {
    // if a; then b; fi; echo
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi),
        kw("fi"), Token::Op(Operator::Semi), w_tok("echo"),
    ]).unwrap().unwrap();
    assert!(matches!(seq.first, Command::If(_)));
    assert_eq!(seq.rest.len(), 1);
    assert_eq!(seq.rest[0].0, Connector::Semi);
    assert!(matches!(seq.rest[0].1, Command::Pipeline(_)));
}

#[test]
fn parse_if_joined_with_and() {
    // if a; then b; fi && echo
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi),
        kw("fi"), Token::Op(Operator::And), w_tok("echo"),
    ]).unwrap().unwrap();
    assert_eq!(seq.rest[0].0, Connector::And);
}

#[test]
fn parse_nested_if() {
    // if a; then if b; then c; fi; fi
    let seq = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"),
        kw("if"), w_tok("b"), Token::Op(Operator::Semi),
        kw("then"), w_tok("c"), Token::Op(Operator::Semi),
        kw("fi"), Token::Op(Operator::Semi),
        kw("fi"),
    ]).unwrap().unwrap();
    let c = first_if(&seq);
    assert!(matches!(c.then_body.first, Command::If(_)));
}

#[test]
fn parse_if_unterminated_is_error() {
    // if a; then b   (no fi)
    let r = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"),
    ]);
    assert_eq!(r, Err(ParseError::UnterminatedIf));
}

#[test]
fn parse_if_missing_then_is_error() {
    // if a; fi
    let r = parse(vec![
        kw("if"), w_tok("a"), Token::Op(Operator::Semi), kw("fi"),
    ]);
    assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))));
}

#[test]
fn parse_bare_then_is_unexpected_keyword() {
    let r = parse(vec![kw("then"), w_tok("x")]);
    assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))));
}

#[test]
fn parse_bare_fi_is_unexpected_keyword() {
    let r = parse(vec![kw("fi")]);
    assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))));
}

#[test]
fn parse_if_empty_condition_is_missing_command() {
    // if ; then b; fi
    let r = parse(vec![
        kw("if"), Token::Op(Operator::Semi),
        kw("then"), w_tok("b"), Token::Op(Operator::Semi), kw("fi"),
    ]);
    assert_eq!(r, Err(ParseError::MissingCommand));
}

#[test]
fn parse_keyword_as_argument_is_literal() {
    // echo if  — `if` is an argument, not a keyword.
    let seq = parse(vec![w_tok("echo"), w_tok("if")]).unwrap().unwrap();
    assert_eq!(seq.first, Command::Pipeline(Pipeline {
        commands: vec![plain("echo", &["if"])],
    }));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test parse_simple_if`
Expected: FAIL — `UnterminatedIf`/`UnexpectedKeyword` don't exist; `if` is parsed as a plain command.

- [ ] **Step 3: Add the new `ParseError` variants**

In `src/command.rs`, extend `ParseError`:

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
}
```

- [ ] **Step 4: Add keyword recognition**

Add to `src/command.rs` (near the top, after the imports):

```rust
use crate::lexer::WordPart;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Keyword {
    If,
    Then,
    Elif,
    Else,
    Fi,
}

impl Keyword {
    fn name(self) -> &'static str {
        match self {
            Keyword::If => "if",
            Keyword::Then => "then",
            Keyword::Elif => "elif",
            Keyword::Else => "else",
            Keyword::Fi => "fi",
        }
    }
}

/// Returns the keyword a token represents, or `None`. A token is a
/// keyword only when it is a `Word` of exactly one part — an *unquoted*
/// `Literal` whose text equals the keyword. So `'if'`, `"if"`, `i"f"`,
/// and `if` used as an argument all stay ordinary words.
fn keyword_of(token: &Token) -> Option<Keyword> {
    let Token::Word(Word(parts)) = token else { return None };
    if parts.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &parts[0] else {
        return None;
    };
    match text.as_str() {
        "if" => Some(Keyword::If),
        "then" => Some(Keyword::Then),
        "elif" => Some(Keyword::Elif),
        "else" => Some(Keyword::Else),
        "fi" => Some(Keyword::Fi),
        _ => None,
    }
}
```

(If `WordPart` is already imported in `command.rs`, do not duplicate the `use`.)

- [ ] **Step 5: Rewrite the parse layer**

Replace the `parse` function body and add `parse_sequence`, `parse_command`, `parse_if`, `expect_keyword`. Keep `parse_pipeline` exactly as it is.

```rust
pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }
    let mut iter = tokens.into_iter().peekable();
    let seq = parse_sequence(&mut iter, &[])?;
    Ok(Some(seq))
}

/// Parses commands joined by `;` / `&&` / `||` (and an optional trailing
/// `&` at top level only). Stops — without consuming — when the next
/// token is a keyword in `stop_at`. `stop_at` is empty only at the top
/// level; a non-empty `stop_at` means we are inside an `if`.
fn parse_sequence<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    stop_at: &[Keyword],
) -> Result<Sequence, ParseError> {
    let at_top_level = stop_at.is_empty();
    let first = parse_command(iter)?;
    let mut rest = Vec::new();
    let mut background = false;

    loop {
        match iter.peek() {
            None => break,
            Some(tok) => {
                if let Some(kw) = keyword_of(tok) {
                    if stop_at.contains(&kw) {
                        break;
                    }
                }
            }
        }
        let token = iter.next().unwrap();
        match token {
            Token::Op(Operator::Background) => {
                if !at_top_level {
                    // `&` inside an `if` condition/body is not supported.
                    return Err(ParseError::UnexpectedBackground);
                }
                if !rest.is_empty() {
                    return Err(ParseError::BackgroundedMultiPipelineSequence);
                }
                if iter.peek().is_some() {
                    return Err(ParseError::UnexpectedBackground);
                }
                background = true;
                break;
            }
            Token::Op(Operator::Semi) => {
                // A trailing `;`, or a `;` right before a stop keyword or
                // end-of-input, simply ends the sequence.
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
                rest.push((Connector::And, parse_command(iter)?));
            }
            Token::Op(Operator::Or) => {
                rest.push((Connector::Or, parse_command(iter)?));
            }
            other => unreachable!("unexpected token after a command: {other:?}"),
        }
    }

    Ok(Sequence { first, rest, background })
}

/// Parses a single sequence element: an `if` clause or a pipeline.
fn parse_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    match iter.peek().and_then(keyword_of) {
        Some(Keyword::If) => Ok(Command::If(Box::new(parse_if(iter)?))),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => Ok(Command::Pipeline(parse_pipeline(iter)?)),
    }
}

/// Consumes one token and checks it is the expected keyword.
fn expect_keyword<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    expected: Keyword,
) -> Result<(), ParseError> {
    match iter.next() {
        Some(ref t) if keyword_of(t) == Some(expected) => Ok(()),
        _ => Err(ParseError::UnterminatedIf),
    }
}

/// Parses `if LIST; then LIST; [elif LIST; then LIST;]... [else LIST;] fi`.
/// The caller has already peeked the leading `if`.
fn parse_if<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<IfClause, ParseError> {
    expect_keyword(iter, Keyword::If)?;
    let condition = parse_sequence(iter, &[Keyword::Then])?;
    expect_keyword(iter, Keyword::Then)?;
    let then_body = parse_sequence(iter, &[Keyword::Elif, Keyword::Else, Keyword::Fi])?;

    let mut elif_branches = Vec::new();
    while iter.peek().and_then(keyword_of) == Some(Keyword::Elif) {
        iter.next(); // consume `elif`
        let condition = parse_sequence(iter, &[Keyword::Then])?;
        expect_keyword(iter, Keyword::Then)?;
        let body = parse_sequence(iter, &[Keyword::Elif, Keyword::Else, Keyword::Fi])?;
        elif_branches.push(ElifBranch { condition, body });
    }

    let else_body = if iter.peek().and_then(keyword_of) == Some(Keyword::Else) {
        iter.next(); // consume `else`
        Some(parse_sequence(iter, &[Keyword::Fi])?)
    } else {
        None
    };

    expect_keyword(iter, Keyword::Fi)?;
    Ok(IfClause { condition, then_body, elif_branches, else_body })
}
```

- [ ] **Step 6: Run the new tests**

Run: `cargo test`
Expected: all parser tests pass, including the 14 new `if` tests; the 620 pre-existing tests still pass (the recursive-descent rewrite is behavior-preserving for non-`if` input). New total ≈ 634.

Note: do NOT execute an `if` anywhere — the executor still `unreachable!`s on `Command::If`. The new tests only call `parse` and inspect the AST.

- [ ] **Step 7: Commit**

```bash
git add src/command.rs
git commit -m "v17 task 2: keyword-aware recursive-descent parser with parse_if"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/if-control-flow`
- Baseline: 620 tests passing
- `parse_pipeline` is unchanged — it needs no keyword awareness. Keywords act as keywords only in command position, which `parse_sequence`/`parse_command` handle.
- `&` is rejected inside an `if` (non-empty `stop_at`) — a v17 limitation noted in the plan header.
- The existing background/multi-pipeline error tests must still pass — the new `parse_sequence` reproduces that logic for the top-level (`stop_at` empty) case.

## Self-Review

- Do all 14 new `if` parser tests pass?
- Do all 620 pre-existing tests still pass (especially the background and sequencing-error tests)?
- Does `echo if` keep `if` as an argument?
- No `if` is executed anywhere in the tests?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 3: Executor — `run_if`

Implement `run_if` and replace the `unreachable!` in `run_command`.

**Files:**
- Modify: `src/executor.rs`

- [ ] **Step 1: Write failing executor tests**

`src/executor.rs` tests use `execute_capturing` (runs a sequence, captures stdout, returns `(String, i32)`). Add to the `tests` module. First a helper to build an `if` sequence:

```rust
use crate::command::{Command, IfClause};

/// A top-level sequence wrapping a single Command.
fn seq_of(cmd: Command) -> Sequence {
    Sequence { first: cmd, rest: vec![], background: false }
}

/// A one-pipeline Sequence running `echo <word>`.
fn echo_seq(word: &str) -> Sequence {
    use crate::command::{ExecCommand, Pipeline};
    use crate::lexer::{Word, WordPart};
    let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
    Sequence {
        first: Command::Pipeline(Pipeline {
            commands: vec![SimpleCommand::Exec(ExecCommand {
                program: ww("echo"),
                args: vec![ww(word)],
                stdin: None,
                stdout: None,
                stderr: None,
            })],
        }),
        rest: vec![],
        background: false,
    }
}

/// A one-pipeline condition Sequence with a known exit status: true
/// (exit 0) when `succeed`, false (exit 1) otherwise. Built from the
/// side-effect-free `test` builtin — `test 0 -eq 0` succeeds,
/// `test 1 -eq 0` fails.
fn cond_seq(succeed: bool) -> Sequence {
    use crate::command::{ExecCommand, Pipeline};
    use crate::lexer::{Word, WordPart};
    let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
    let lhs = if succeed { "0" } else { "1" };
    Sequence {
        first: Command::Pipeline(Pipeline {
            commands: vec![SimpleCommand::Exec(ExecCommand {
                program: ww("test"),
                args: vec![ww(lhs), ww("-eq"), ww("0")],
                stdin: None,
                stdout: None,
                stderr: None,
            })],
        }),
        rest: vec![],
        background: false,
    }
}
```

Then the tests:

```rust
#[test]
fn if_true_condition_runs_then_body() {
    let clause = IfClause {
        condition: cond_seq(true),
        then_body: echo_seq("yes"),
        elif_branches: vec![],
        else_body: None,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
    assert_eq!(out.trim(), "yes");
    assert_eq!(status, 0);
}

#[test]
fn if_false_condition_runs_else_body() {
    let clause = IfClause {
        condition: cond_seq(false),
        then_body: echo_seq("yes"),
        elif_branches: vec![],
        else_body: Some(echo_seq("no")),
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
    assert_eq!(out.trim(), "no");
}

#[test]
fn if_false_no_else_runs_nothing_status_zero() {
    let clause = IfClause {
        condition: cond_seq(false),
        then_body: echo_seq("yes"),
        elif_branches: vec![],
        else_body: None,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn if_elif_selects_matching_branch() {
    use crate::command::ElifBranch;
    // if (false); then a; elif (true); then b; else c; fi
    let clause = IfClause {
        condition: cond_seq(false),
        then_body: echo_seq("a"),
        elif_branches: vec![ElifBranch {
            condition: cond_seq(true),
            body: echo_seq("b"),
        }],
        else_body: Some(echo_seq("c")),
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&seq_of(Command::If(Box::new(clause))), &mut shell);
    assert_eq!(out.trim(), "b");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test if_true_condition_runs_then_body`
Expected: FAIL — `run_command`'s `Command::If` arm is still `unreachable!` (panics).

- [ ] **Step 3: Implement `run_if` and wire it in**

In `src/executor.rs`, replace the `Command::If(_) => unreachable!(...)` arm of `run_command`:

```rust
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink),
        Command::If(clause) => run_if(clause, shell, sink),
    }
}

/// Runs an `if` clause: evaluate the condition, then run the first
/// branch whose condition succeeds (exit 0), or the `else` body, or
/// nothing (status 0). An `exit` anywhere inside propagates.
fn run_if(clause: &IfClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let cond = execute_sequence_body(&clause.condition, shell, sink);
    if matches!(cond, ExecOutcome::Exit(_)) {
        return cond;
    }
    if matches!(cond, ExecOutcome::Continue(0)) {
        return execute_sequence_body(&clause.then_body, shell, sink);
    }
    for elif in &clause.elif_branches {
        let elif_cond = execute_sequence_body(&elif.condition, shell, sink);
        if matches!(elif_cond, ExecOutcome::Exit(_)) {
            return elif_cond;
        }
        if matches!(elif_cond, ExecOutcome::Continue(0)) {
            return execute_sequence_body(&elif.body, shell, sink);
        }
    }
    if let Some(else_body) = &clause.else_body {
        return execute_sequence_body(else_body, shell, sink);
    }
    ExecOutcome::Continue(0)
}
```

`IfClause` is already imported (added in Task 1's `use`). If not, add it.

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: all tests pass — the 4 new executor tests plus everything else (≈ 638).

- [ ] **Step 5: Manual smoke test**

```bash
cargo build --release
~/projects/shuck/target/release/huck <<'EOF'
if test -d /tmp; then echo tmp-exists; else echo no-tmp; fi
if test -f /no/such/path; then echo found; else echo missing; fi
if test 1 -eq 2; then echo a; elif test 2 -eq 2; then echo b; else echo c; fi
if test -d /tmp; then echo yes; fi && echo chained
x=5; if test "$x" -gt 3; then echo big; fi
exit
EOF
```

Expected output: `tmp-exists`, `missing`, `b`, `yes`, `chained`, `big`.

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs
git commit -m "v17 task 3: executor run_if — if/elif/else execution"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/if-control-flow`
- Baseline: ≈634 tests passing (after Task 2)
- `execute_sequence_body` is the sub-sequence runner; `run_if` calls it for the condition and each branch. The mutual recursion `run_command` → `run_if` → `execute_sequence_body` → `run_command` is fine in Rust.
- A plain `if` runs in the shell's own process. `test` (v16) in the condition runs as an in-process builtin.
- An `exit` inside the condition or a branch produces `ExecOutcome::Exit`, which `run_if` propagates so the shell terminates.

## Self-Review

- Do all 4 new executor tests pass?
- Does the smoke test produce exactly the expected six lines?
- Do all pre-existing tests still pass?

## Report Format

Status, smoke test output, test count, commit SHA, any concerns.

---

## Task 4: Parse-error display

Render the two new `ParseError` variants for the user.

**Files:**
- Modify: `src/shell.rs`

- [ ] **Step 1: Find the `ParseError` display path**

In `src/shell.rs`, find where `ParseError` is turned into a user-facing message (a `match` on `ParseError`, alongside the existing `MissingCommand` / `MissingRedirectTarget` / etc. arms). It produces a string printed after `huck: syntax error` or similar.

- [ ] **Step 2: Add arms for the new variants**

Add arms matching the existing style:

```rust
ParseError::UnterminatedIf => ": unterminated 'if' (expected 'then'/'fi')".to_string(),
ParseError::UnexpectedKeyword(kw) => format!(": unexpected '{kw}'"),
```

Adapt the exact wording/format to match the surrounding arms (e.g. whether they include a leading `: ` or a prefix). The goal: `if x; then y` prints something like `huck: syntax error: unterminated 'if' (expected 'then'/'fi')` and `then` alone prints `huck: syntax error: unexpected 'then'`.

- [ ] **Step 3: Build and run the full suite**

Run: `cargo build && cargo test`
Expected: all tests pass; the `match` on `ParseError` is now exhaustive.

- [ ] **Step 4: Commit**

```bash
git add src/shell.rs
git commit -m "v17 task 4: display the new if-related parse errors"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/if-control-flow`
- The `match` on `ParseError` in `shell.rs` must become exhaustive — the compiler will flag the two missing arms if you forget either.

## Report Format

Status, the message format you used, commit SHA, any concerns.

---

## Task 5: End-to-end integration tests

**Files:**
- Create: `tests/if_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/if_integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn if_then_taken_for_true_condition() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("present");
    std::fs::write(&file, b"x").unwrap();
    let script = format!(
        "if test -f '{}'; then echo yes; else echo no; fi\nexit\n",
        file.to_str().unwrap()
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l == "yes"), "stdout: {out}");
}

#[test]
fn if_else_taken_for_false_condition() {
    let (out, _) = run("if test -f /no/such/huck/path; then echo yes; else echo no; fi\nexit\n");
    assert!(out.lines().any(|l| l == "no"), "stdout: {out}");
}

#[test]
fn elif_chain_selects_middle_branch() {
    let (out, _) = run(
        "if test 1 -eq 2; then echo a; elif test 2 -eq 2; then echo b; else echo c; fi\nexit\n",
    );
    assert!(out.lines().any(|l| l == "b"), "stdout: {out}");
}

#[test]
fn if_multi_command_body() {
    let (out, _) = run("if test 1 -eq 1; then echo one; echo two; fi\nexit\n");
    assert!(out.lines().any(|l| l == "one"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "two"), "stdout: {out}");
}

#[test]
fn if_chained_with_and() {
    let (out, _) = run("if test 1 -eq 1; then echo body; fi && echo chained\nexit\n");
    assert!(out.lines().any(|l| l == "body"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "chained"), "stdout: {out}");
}

#[test]
fn nested_if() {
    let (out, _) = run(
        "if test 1 -eq 1; then if test 2 -eq 2; then echo deep; fi; fi\nexit\n",
    );
    assert!(out.lines().any(|l| l == "deep"), "stdout: {out}");
}

#[test]
fn if_status_reflects_branch() {
    // The then-body's last command is `false`-like; $? after the if is 1.
    let (out, _) = run("if test 1 -eq 1; then test 1 -eq 2; fi\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
}

#[test]
fn if_no_else_no_match_status_zero() {
    let (out, _) = run("if test 1 -eq 2; then echo a; fi\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "0"), "stdout: {out}");
}

#[test]
fn unterminated_if_is_syntax_error() {
    let (_, err) = run("if test 1 -eq 1; then echo x\nexit\n");
    assert!(err.to_lowercase().contains("syntax error"), "stderr: {err}");
}

#[test]
fn echo_if_prints_if() {
    let (out, _) = run("echo if\nexit\n");
    assert!(out.lines().any(|l| l == "if"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test if_integration`
Expected: 10 tests pass.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: all tests pass (≈ 648).

- [ ] **Step 4: Commit**

```bash
git add tests/if_integration.rs
git commit -m "v17 task 5: end-to-end if integration tests"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/if-control-flow`
- `test` (v16) is the natural condition command. `tempfile` is a dev-dependency.
- `unterminated_if_is_syntax_error` feeds a script whose first line is an incomplete `if`; huck reads input line by line, so the line fails to parse — confirm the error text matches whatever Task 4 produces and adjust the assertion if needed.

## Self-Review

- Do all 10 integration tests pass?
- Does `echo if` still print `if` (the regression case)?
- Does the full suite pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 6: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v17 row to the status table**

Append after the v16 row:

```
| v17       | `if` control flow (`if`/`elif`/`else`/`fi`)             |
```

Match the table's column alignment.

- [ ] **Step 2: Add a features note**

After the v16 Conditionals block, add:

```markdown
**`if` control flow (v17):**
`if LIST; then LIST; [elif LIST; then LIST;]... [else LIST;] fi`
runs the `then` body when the condition's exit status is 0, an
`elif` body when its condition succeeds, or the `else` body. An `if`
is a compound command at the sequence level: it composes with `;`,
`&&`, `||`, nests inside branch bodies, and can be followed by more
commands. Single-line form only (parts separated by `;`); multi-line
`if`, `if` inside a `|` pipeline, and backgrounding a whole `if` are
not yet implemented.
```

- [ ] **Step 3: Update the Not-yet-implemented section**

The list mentions `control flow (if/while/for/case)`. Update it so `if` is removed and the remaining control flow reads `control flow (while/until/for/case)`.

- [ ] **Step 4: Update the test count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the `passed` counts. Update the `cargo test               # full test suite (NNN tests)` line. Expected ≈ 648 — use the actual number.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "v17 task 6: README — add v17 row and if-control-flow section"
```

---

## Final review checkpoint

After Task 6:

- [ ] `cargo test` shows the expected total passing, 0 failing
- [ ] `cargo clippy -- -D warnings` is clean (or any new warnings are intentional)
- [ ] Manual REPL smoke session: `if`/`then`/`fi`, `if`/`else`, an `elif` chain, a nested `if`, `if ...; fi && echo`, `if ...; fi; echo`, a syntax error (`if x; then y`), and `echo if`
- [ ] Final review the whole branch as a single diff before merging to main
