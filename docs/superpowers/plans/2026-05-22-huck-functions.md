# huck v22: Functions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add user-defined functions to huck — `name() { … }` definition, `f arg1 arg2` invocation, positional parameters (`$1`-`$N`, `$@`, `$*`, `$#`), and the `return` builtin.

**Architecture:** Five mutually-dependent pieces land in order: brace groups (`{ … }` — the function-body form); the `ExecOutcome::FunctionReturn` variant + `return` builtin (the propagation channel); the function-definition AST and parser (`Command::FunctionDef`); positional parameters as an expansion construct (`WordPart::AllArgs`, `Shell::positional_args`, `lookup_var`); and finally the function-call mechanism in the executor (function table on `Shell`, scope management, dispatch precedence).

**Tech Stack:** Rust 2024, `glob` crate (already a dependency).

---

## File Map

| File | Change | Responsibility |
| --- | --- | --- |
| `src/command.rs` | Modify | Brace-group + function-def AST, `parse_brace_group`, `parse_pipeline_with_first` refactor, function-def detection in `parse_command`, new errors, keyword additions |
| `src/continuation.rs` | Modify | Classify `UnterminatedBrace`; joiner knows `{` |
| `src/lexer.rs` | Modify | `$N` (digit) / `$@` / `$*` / `$#` / `${N}` / `${@}` / `${*}` / `${#}` recognition; emit `WordPart::AllArgs` |
| `src/expand.rs` | Modify | `WordPart::AllArgs` arms in `expand` / `expand_assignment` / `expand_pattern`; positional lookup via `Shell::lookup_var`; `word_part_is_quoted` updated |
| `src/shell_state.rs` | Modify | `functions: HashMap<…>`, `positional_args: Vec<String>`, `lookup_var` accessor |
| `src/builtins.rs` | Modify | `ExecOutcome::FunctionReturn(i32)`; `return` builtin |
| `src/executor.rs` | Modify | `run_command` arms for `BraceGroup` and `FunctionDef`; function-call dispatch in `run_exec_single`; `call_function` helper; propagate `FunctionReturn` through all match sites |
| `src/shell.rs` | Modify | REPL extends stray-loop-control neutralisation to also handle `FunctionReturn`; new error-message arms |
| `tests/functions_integration.rs` | Create | End-to-end function scripts |
| `README.md` | Modify | v22 status row, feature note, remove `functions` from "Not yet implemented" |

**Baseline:** `cargo test` passes 814 tests before this work begins (after the warning-cleanup commit `727cfcb`).

---

## Task 1: Brace groups `{ … }`

**Files:**
- Modify: `src/command.rs`
- Modify: `src/executor.rs`
- Modify: `src/shell.rs`
- Modify: `src/continuation.rs`

The lexer is **unchanged** — `{`/`}` stay ordinary word characters. POSIX requires them as separate words (whitespace-separated), so `{ cmd; }` lexes to `Word("{")`, `Word("cmd")`, `Op(Semi)`, `Word("}")` and `keyword_of` recognises the standalone `"{"`/`"}"` words as keywords.

- [ ] **Step 1: Write the failing parser tests**

In `src/command.rs`'s `#[cfg(test)] mod tests`, add (the existing `kw`/`w_tok`/`plain` helpers are reused; `kw` aliases `w_tok`):

```rust
#[test]
fn parse_brace_group_simple() {
    // { echo hi ; }
    let seq = parse(vec![
        kw("{"), w_tok("echo"), w_tok("hi"), Token::Op(Operator::Semi), kw("}"),
    ]).unwrap().unwrap();
    let body = match &seq.first {
        Command::BraceGroup(b) => b.as_ref(),
        other => panic!("expected a brace group, got {other:?}"),
    };
    assert_eq!(body.first, Command::Pipeline(Pipeline { commands: vec![plain("echo", &["hi"])] }));
}

#[test]
fn parse_brace_group_multiline_matches_singleline() {
    let multi = parse(vec![
        kw("{"), Token::Newline, w_tok("echo"), Token::Newline, kw("}"),
    ]).unwrap().unwrap();
    let single = parse(vec![
        kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
    ]).unwrap().unwrap();
    assert_eq!(multi, single);
}

#[test]
fn parse_brace_group_unterminated() {
    // missing `}`
    assert_eq!(
        parse(vec![kw("{"), w_tok("echo"), Token::Op(Operator::Semi)]),
        Err(ParseError::UnterminatedBrace)
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib parse_brace_group_simple`
Expected: FAIL — compile error: `Command::BraceGroup`, `ParseError::UnterminatedBrace` do not exist; `Keyword::LBrace`/`RBrace` do not exist.

- [ ] **Step 3: Add keywords**

In `src/command.rs`, extend the `Keyword` enum — add `LBrace` and `RBrace` after `Esac`. Add to `Keyword::name`:

```rust
            Keyword::LBrace => "{",
            Keyword::RBrace => "}",
```

Add to `keyword_of`:

```rust
        "{" => Some(Keyword::LBrace),
        "}" => Some(Keyword::RBrace),
```

- [ ] **Step 4: Add the AST and error**

Extend `Command`:

```rust
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
    While(Box<WhileClause>),
    For(Box<ForClause>),
    Case(Box<CaseClause>),
    BraceGroup(Box<Sequence>),
}
```

Extend `ParseError`:

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
    UnterminatedBrace,
}
```

- [ ] **Step 5: Add `parse_brace_group` and wire it into `parse_command`**

In `src/command.rs`, add next to `parse_case`:

```rust
/// Parses `{ LIST }`. The caller has peeked the leading `{`.
fn parse_brace_group<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Sequence, ParseError> {
    expect_keyword(iter, Keyword::LBrace, ParseError::UnterminatedBrace)?;
    let body = parse_compound_section(iter, &[Keyword::RBrace], ParseError::UnterminatedBrace)?;
    expect_keyword(iter, Keyword::RBrace, ParseError::UnterminatedBrace)?;
    Ok(body)
}
```

In `parse_command`, add an arm in the compound-keyword dispatch (after the `Case` arm, before `Some(other)`):

```rust
        Some(Keyword::LBrace) => Ok(Command::BraceGroup(Box::new(parse_brace_group(iter)?))),
```

- [ ] **Step 6: Add the executor arm and the test-helper arms**

In `src/executor.rs`, `run_command`'s `match cmd` gains:

```rust
        Command::BraceGroup(seq) => execute_sequence_body(seq, shell, sink),
```

In `src/command.rs`'s test module, the helpers `first_pipeline`, `first_if`, `first_for`, `first_case` have exhaustive `match`es on `&seq.first`. Add a `BraceGroup` arm to each (matching the existing style):

```rust
            Command::BraceGroup(_) => panic!("expected a pipeline, got a brace group"),
```
```rust
            Command::BraceGroup(_) => panic!("expected an if, got a brace group"),
```
```rust
            Command::BraceGroup(_) => panic!("expected a for, got a brace group"),
```
```rust
            Command::BraceGroup(_) => panic!("expected a case, got a brace group"),
```

(`first_while` uses `other => panic!` catch-all and needs no change.)

- [ ] **Step 7: Add the error message in `shell.rs`**

In `src/shell.rs`, `parse_error_message`'s match adds:

```rust
        ParseError::UnterminatedBrace => "unterminated '{' (expected '}')".to_string(),
```

- [ ] **Step 8: Teach the continuation classifier**

In `src/continuation.rs`:

`classify`'s parse-error match (currently `Err(UnterminatedIf | UnterminatedLoop | UnterminatedCase)`) adds `UnterminatedBrace`:

```rust
        Err(ParseError::UnterminatedIf
            | ParseError::UnterminatedLoop
            | ParseError::UnterminatedCase
            | ParseError::UnterminatedBrace) => {
            Completeness::Incomplete(ContinuationReason::Compound)
        }
```

`ends_with_control_keyword`'s `matches!` gains `"{"`:

```rust
        Some("if" | "while" | "until" | "then" | "do" | "else" | "elif" | "for" | "in" | "case" | "{")
```

Add two tests:

```rust
#[test]
fn unterminated_brace_is_incomplete() {
    assert_eq!(
        classify("{ echo hi"),
        Completeness::Incomplete(ContinuationReason::Compound)
    );
}

#[test]
fn joiner_compound_is_space_after_open_brace() {
    assert_eq!(joiner_for(ContinuationReason::Compound, "{"), " ");
}
```

- [ ] **Step 9: Add an executor unit test for "runs in current shell"**

In `src/executor.rs`'s `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn brace_group_assignments_affect_current_shell() {
    // A brace group has NO subshell isolation — `x=value` inside it
    // is visible after.
    let assign = Sequence {
        first: Command::Pipeline(Pipeline {
            commands: vec![SimpleCommand::Assign {
                name: "BG_X".to_string(),
                value: Word(vec![WordPart::Literal { text: "hello".to_string(), quoted: false }]),
            }],
        }),
        rest: vec![],
        background: false,
    };
    let group = Sequence {
        first: Command::BraceGroup(Box::new(assign)),
        rest: vec![],
        background: false,
    };
    let mut shell = Shell::new();
    let (_, status) = execute_capturing(&group, &mut shell);
    assert_eq!(status, 0);
    assert_eq!(shell.get("BG_X"), Some("hello"));
}
```

- [ ] **Step 10: Run the full suite**

Run: `cargo test`
Expected: PASS — the 3 parser tests + 2 continuation tests + 1 executor test pass, and all 814 prior tests still pass.

If a pre-existing test fails because it fed an unquoted standalone `{` or `}` as a literal word, that's the expected ripple — quote it (`"{"` / `'}'`) so it stays literal. Do NOT change shell behaviour.

- [ ] **Step 11: Commit**

```bash
git add src/command.rs src/executor.rs src/shell.rs src/continuation.rs
git commit -m "v22 task 1: brace groups { … }"
```

---

## Task 2: `ExecOutcome::FunctionReturn` + `return` builtin

**Files:**
- Modify: `src/builtins.rs`
- Modify: `src/executor.rs` (propagation arms — same ripple v18 did for `LoopBreak`)
- Modify: `src/shell.rs` (REPL stray-control neutralisation)

This mirrors v18's `LoopBreak`/`LoopContinue` machinery exactly — a new `ExecOutcome` variant that propagates through every short-circuit site. Without function calls (Task 5), `return` is effectively a no-op (the variant propagates to the top level and the REPL neutralises it), but the plumbing must be in place before the function-call site can catch it.

- [ ] **Step 1: Write the failing builtin tests**

In `src/builtins.rs`'s `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn builtin_return_with_arg_returns_function_return() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        run_builtin("return", &["7".to_string()], &mut out, &mut shell),
        ExecOutcome::FunctionReturn(7)
    );
}

#[test]
fn builtin_return_no_arg_returns_last_status() {
    let mut shell = Shell::new();
    shell.set_last_status(42);
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        run_builtin("return", &[], &mut out, &mut shell),
        ExecOutcome::FunctionReturn(42)
    );
}

#[test]
fn builtin_return_invalid_arg_falls_back_to_last_status() {
    let mut shell = Shell::new();
    shell.set_last_status(13);
    let mut out: Vec<u8> = Vec::new();
    assert_eq!(
        run_builtin("return", &["not-a-num".to_string()], &mut out, &mut shell),
        ExecOutcome::FunctionReturn(13)
    );
}
```

If the existing test module uses a different sink type (e.g. `std::io::sink()`), match that style — the key is calling `run_builtin("return", &args, sink, &mut shell)`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib builtin_return_with_arg_returns_function_return`
Expected: FAIL — `ExecOutcome::FunctionReturn` does not exist; `"return"` is not in `BUILTIN_NAMES`.

- [ ] **Step 3: Add the `ExecOutcome` variant**

In `src/builtins.rs`, extend the `ExecOutcome` enum:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak,
    LoopContinue,
    FunctionReturn(i32),
}
```

- [ ] **Step 4: Add `return` to the builtin set**

In `src/builtins.rs`, add `"return"` to `BUILTIN_NAMES`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return",
];
```

In `run_builtin`'s dispatch, add the `"return"` arm next to `"break"`/`"continue"`:

```rust
        "return" => {
            let code = match args.first() {
                Some(s) => s.parse::<i32>().unwrap_or_else(|_| shell.last_status()),
                None => shell.last_status(),
            };
            ExecOutcome::FunctionReturn(code)
        }
```

- [ ] **Step 5: Ripple `FunctionReturn` through every `ExecOutcome` match site**

Adding the variant makes every exhaustive `match` on `ExecOutcome` non-exhaustive. The compiler will list each site; the rule is: `FunctionReturn` propagates *out* of every loop / `if` / `case` / sequence — same handling as `Exit`. Specifically:

In `src/executor.rs`:

- `execute_sequence_body`'s short-circuit checks: add `FunctionReturn` alongside `Exit` so a sequence stops and propagates it. Search for `ExecOutcome::Exit(_)` patterns combined with `LoopBreak | LoopContinue` and add `FunctionReturn(_)` to those groups.
- `run_if`'s condition-result match (two sites — main condition and elif condition): treat `FunctionReturn` like `Exit` — propagate.
- `run_while`'s body-result match: `LoopBreak`/`LoopContinue` are caught; `FunctionReturn` is NOT caught — propagate:

  ```rust
          match execute_sequence_body(&clause.body, shell, sink) {
              ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
              ExecOutcome::FunctionReturn(code) => return ExecOutcome::FunctionReturn(code),
              ExecOutcome::LoopBreak => { last = ExecOutcome::Continue(0); break; }
              ExecOutcome::LoopContinue => { last = ExecOutcome::Continue(0); }
              ExecOutcome::Continue(c) => { last = ExecOutcome::Continue(c); }
          }
  ```

  And the condition-result match in `run_while` (top of loop): propagate `FunctionReturn` like `Exit`.

- `run_for`'s body-result match: same pattern — propagate `FunctionReturn`.
- `run_case`'s body-result match: same pattern — propagate `FunctionReturn`.
- `execute_capturing` (test helper): an `ExecOutcome::FunctionReturn(n)` at the top level should be treated like `Continue(n)` for status purposes — add the arm. Find the existing `LoopBreak | LoopContinue => 0` arm and extend:

  ```rust
          ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => 0,
          ExecOutcome::FunctionReturn(n) => n,
  ```

- `run_background_sequence`'s pure-builtin path and `run_multi_stage`'s builtin-stage path: extend the `match` that converts `ExecOutcome` to an exit code with a `FunctionReturn(n) => n` arm (treat like `Continue(n)`).

In `src/shell.rs`, the REPL's `match process_line(...)` has an arm:

```rust
                    ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => {
                        shell.set_last_status(0)
                    }
```

Extend it:

```rust
                    ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
                    | ExecOutcome::FunctionReturn(_) => {
                        shell.set_last_status(0)
                    }
```

If the compiler flags any other `match` on `ExecOutcome` not listed above, add a `FunctionReturn` arm following the same propagate-like-Exit rule, and note it in the report.

- [ ] **Step 6: Run the tests**

Run: `cargo test`
Expected: PASS — the 3 new builtin tests pass and all 814 prior tests still pass. The propagation is correct: `LoopBreak`/`LoopContinue`'s behaviour is unchanged.

- [ ] **Step 7: Commit**

```bash
git add src/builtins.rs src/executor.rs src/shell.rs
git commit -m "v22 task 2: ExecOutcome::FunctionReturn machinery and return builtin"
```

---

## Task 3: Function-definition AST & parser

**Files:**
- Modify: `src/command.rs`
- Modify: `src/shell.rs` (error messages)
- Modify: `src/executor.rs` (placeholder arm)

This adds the function-definition AST (`Command::FunctionDef`), refactors `parse_pipeline` to allow a pre-consumed first word, and adds the function-def detection in `parse_command`. The executor only gets a placeholder — Task 5 wires the real execution.

- [ ] **Step 1: Write the failing parser tests**

In `src/command.rs`'s `#[cfg(test)] mod tests`, add a `first_function` helper next to `first_case`, and the tests:

```rust
fn first_function(seq: &Sequence) -> (&str, &Command) {
    match &seq.first {
        Command::FunctionDef { name, body } => (name.as_str(), body.as_ref()),
        other => panic!("expected a function def, got {other:?}"),
    }
}

#[test]
fn parse_simple_function_def() {
    // foo() { echo hi; }
    let seq = parse(vec![
        w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
        kw("{"), w_tok("echo"), w_tok("hi"), Token::Op(Operator::Semi), kw("}"),
    ]).unwrap().unwrap();
    let (name, body) = first_function(&seq);
    assert_eq!(name, "foo");
    assert!(matches!(body, Command::BraceGroup(_)));
}

#[test]
fn parse_function_with_if_body() {
    // foo() if true; then echo; fi
    let seq = parse(vec![
        w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
        kw("if"), w_tok("true"), Token::Op(Operator::Semi),
        kw("then"), w_tok("echo"), Token::Op(Operator::Semi),
        kw("fi"),
    ]).unwrap().unwrap();
    let (name, body) = first_function(&seq);
    assert_eq!(name, "foo");
    assert!(matches!(body, Command::If(_)));
}

#[test]
fn parse_function_invalid_name() {
    // 1foo() { echo; }
    assert_eq!(
        parse(vec![
            w_tok("1foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
            kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
        ]),
        Err(ParseError::FunctionName)
    );
}

#[test]
fn parse_function_missing_close_paren() {
    // foo( { echo; }
    assert_eq!(
        parse(vec![
            w_tok("foo"), Token::Op(Operator::LParen),
            kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
        ]),
        Err(ParseError::FunctionBody)
    );
}

#[test]
fn parse_function_pipeline_body_errors() {
    // foo() echo hi  — body is a Pipeline, not a compound
    assert_eq!(
        parse(vec![
            w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
            w_tok("echo"), w_tok("hi"),
        ]),
        Err(ParseError::FunctionBody)
    );
}

#[test]
fn parse_function_def_followed_by_call() {
    // foo() { echo; } ; foo
    let seq = parse(vec![
        w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
        kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
        Token::Op(Operator::Semi),
        w_tok("foo"),
    ]).unwrap().unwrap();
    // First is the FunctionDef; rest has one Semi-connected Pipeline calling `foo`.
    assert!(matches!(seq.first, Command::FunctionDef { .. }));
    assert_eq!(seq.rest.len(), 1);
    assert!(matches!(seq.rest[0].1, Command::Pipeline(_)));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib parse_simple_function_def`
Expected: FAIL — compile error: `Command::FunctionDef`, `ParseError::FunctionName`, `ParseError::FunctionBody`, `first_function` do not exist.

- [ ] **Step 3: Add the AST and errors**

In `src/command.rs`, extend `Command`:

```rust
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
    While(Box<WhileClause>),
    For(Box<ForClause>),
    Case(Box<CaseClause>),
    BraceGroup(Box<Sequence>),
    FunctionDef { name: String, body: Box<Command> },
}
```

Extend `ParseError`:

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
    UnterminatedBrace,
    FunctionName,
    FunctionBody,
}
```

- [ ] **Step 4: Extract `valid_identifier_text`**

In `src/command.rs`, find `for_variable_name`. Refactor: extract its validation body into a shared helper `valid_identifier_text(&Word) -> Option<String>`, and have `for_variable_name` wrap it:

```rust
/// Returns the text of `word` if it is a single, unquoted `Literal` whose
/// text is a valid identifier (`[A-Za-z_][A-Za-z0-9_]*`) and is not a
/// reserved keyword. Used by `for`-loop variable names and function names.
fn valid_identifier_text(word: &Word) -> Option<String> {
    if word.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &word.0[0] else {
        return None;
    };
    // Reject reserved keywords. Build a single-Word token to reuse keyword_of.
    let tok = Token::Word(Word(vec![WordPart::Literal {
        text: text.clone(),
        quoted: false,
    }]));
    if keyword_of(&tok).is_some() {
        return None;
    }
    let mut chars = text.chars();
    let first = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(text.clone())
}

fn for_variable_name(token: &Token) -> Option<String> {
    let Token::Word(w) = token else { return None };
    valid_identifier_text(w)
}
```

- [ ] **Step 5: Refactor `parse_pipeline` to accept a pre-consumed first word**

In `src/command.rs`, rename the current `parse_pipeline` to `parse_pipeline_with_first`, give it a new first parameter, and add a one-line wrapper:

```rust
fn parse_pipeline_with_first<I: Iterator<Item = Token>>(
    first: Option<Word>,
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> {
    let mut commands: Vec<SimpleCommand> = Vec::new();

    let mut program: Option<Word> = first;
    let mut args: Vec<Word> = Vec::new();
    let mut stdin: Option<Word> = None;
    let mut stdout: Option<Redirect> = None;
    let mut stderr: Option<Redirect> = None;

    // ... rest of the existing parse_pipeline body, unchanged ...
}

fn parse_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> {
    parse_pipeline_with_first(None, iter)
}
```

The only change is the function signature + the `program` initialiser; the body of the existing function is moved verbatim.

- [ ] **Step 6: Add function-def detection in `parse_command`**

In `src/command.rs`, `parse_command`'s `None` arm (currently `None => Ok(Command::Pipeline(parse_pipeline(iter)?))`) is replaced. The new structure:

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
        Some(Keyword::For) => Ok(Command::For(Box::new(parse_for(iter)?))),
        Some(Keyword::Case) => Ok(Command::Case(Box::new(parse_case(iter)?))),
        Some(Keyword::LBrace) => Ok(Command::BraceGroup(Box::new(parse_brace_group(iter)?))),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => {
            // Non-keyword: may be a function definition `name() compound`, or
            // a plain pipeline. Need two-token lookahead.
            if matches!(iter.peek(), Some(Token::Word(_))) {
                // Consume the word; peek for `(`.
                let Some(Token::Word(w)) = iter.next() else { unreachable!() };
                if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
                    return parse_function_def(w, iter);
                }
                // Not a function def — pipeline with `w` as the first word.
                Ok(Command::Pipeline(parse_pipeline_with_first(Some(w), iter)?))
            } else {
                Ok(Command::Pipeline(parse_pipeline(iter)?))
            }
        }
    }
}

/// Parses `name() compound-command`. The caller has consumed the name
/// (`name`) and verified the next token is `(`.
fn parse_function_def<I: Iterator<Item = Token>>(
    name_word: Word,
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    let name = valid_identifier_text(&name_word).ok_or(ParseError::FunctionName)?;
    // Consume `(`.
    iter.next();
    // Expect `)`.
    match iter.next() {
        Some(Token::Op(Operator::RParen)) => {}
        _ => return Err(ParseError::FunctionBody),
    }
    skip_newlines(iter);
    let body = parse_command(iter)?;
    if matches!(body, Command::Pipeline(_)) {
        return Err(ParseError::FunctionBody);
    }
    Ok(Command::FunctionDef { name, body: Box::new(body) })
}
```

- [ ] **Step 7: Add the placeholder executor arm**

In `src/executor.rs`, `run_command`'s `match cmd` gains:

```rust
        Command::FunctionDef { .. } => unreachable!("function execution lands in v22 task 5"),
```

- [ ] **Step 8: Add error messages in `shell.rs`**

In `src/shell.rs`, `parse_error_message`'s match adds:

```rust
        ParseError::FunctionName => "invalid function name".to_string(),
        ParseError::FunctionBody => {
            "'(' must be followed by ')' and a compound-command body".to_string()
        }
```

- [ ] **Step 9: Update test-helper `match` arms for the new `Command` variant**

In `src/command.rs`'s test module, `first_pipeline`, `first_if`, `first_for`, `first_case` (and the new `first_function` from Step 1) have exhaustive `match`es. Add a `FunctionDef { .. }` arm to each:

```rust
            Command::FunctionDef { .. } => panic!("expected a pipeline, got a function def"),
```

(and the equivalent for the others). The compiler will tell you exactly where.

- [ ] **Step 10: Run the full suite**

Run: `cargo test`
Expected: PASS — the 6 new parser tests + the existing 814 + Task 1's + Task 2's all pass.

- [ ] **Step 11: Commit**

```bash
git add src/command.rs src/executor.rs src/shell.rs
git commit -m "v22 task 3: function-definition AST and parser"
```

---

## Task 4: Positional parameters

**Files:**
- Modify: `src/lexer.rs`
- Modify: `src/expand.rs`
- Modify: `src/shell_state.rs`

Adds `$1`-`$N`, `${N}`, `$@`/`"$@"`, `$*`/`"$*"`, `$#` as expansion constructs, and the `Shell::positional_args` frame they read from. The function-call mechanism that *populates* `positional_args` is Task 5 — for now the field exists, defaults to empty, and is settable in tests.

- [ ] **Step 1: Write the failing lexer tests**

In `src/lexer.rs`'s `#[cfg(test)] mod tests`, add (the existing `w`/`var` helpers exist; check the file for exact names if needed):

```rust
#[test]
fn tokenize_dollar_digit() {
    let tokens = tokenize("$1").unwrap();
    assert_eq!(
        tokens,
        vec![Token::Word(Word(vec![WordPart::Var {
            name: "1".to_string(), quoted: false
        }]))]
    );
}

#[test]
fn tokenize_dollar_hash() {
    let tokens = tokenize("$#").unwrap();
    assert_eq!(
        tokens,
        vec![Token::Word(Word(vec![WordPart::Var {
            name: "#".to_string(), quoted: false
        }]))]
    );
}

#[test]
fn tokenize_dollar_at() {
    let tokens = tokenize("$@").unwrap();
    assert_eq!(
        tokens,
        vec![Token::Word(Word(vec![WordPart::AllArgs {
            joined: false, quoted: false
        }]))]
    );
}

#[test]
fn tokenize_dollar_star() {
    let tokens = tokenize("$*").unwrap();
    assert_eq!(
        tokens,
        vec![Token::Word(Word(vec![WordPart::AllArgs {
            joined: true, quoted: false
        }]))]
    );
}

#[test]
fn tokenize_quoted_dollar_at() {
    let tokens = tokenize("\"$@\"").unwrap();
    assert_eq!(
        tokens,
        vec![Token::Word(Word(vec![WordPart::AllArgs {
            joined: false, quoted: true
        }]))]
    );
}

#[test]
fn tokenize_braced_positional() {
    let tokens = tokenize("${10}").unwrap();
    assert_eq!(
        tokens,
        vec![Token::Word(Word(vec![WordPart::Var {
            name: "10".to_string(), quoted: false
        }]))]
    );
}

#[test]
fn tokenize_braced_special() {
    let tokens = tokenize("${@}").unwrap();
    assert_eq!(
        tokens,
        vec![Token::Word(Word(vec![WordPart::AllArgs {
            joined: false, quoted: false
        }]))]
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib tokenize_dollar_digit`
Expected: FAIL — `WordPart::AllArgs` does not exist; the lexer currently rejects `$@`/`$*`/`$#`/`$<digit>`.

- [ ] **Step 3: Add the `WordPart::AllArgs` variant**

In `src/lexer.rs`, extend `WordPart`:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum WordPart {
    Literal { text: String, quoted: bool },
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
    Arith { expr: crate::arith::ArithExpr, quoted: bool },
    ParamExpansion { name: String, modifier: ParamModifier, quoted: bool },
    AllArgs { quoted: bool, joined: bool },
}
```

- [ ] **Step 4: Extend the lexer's `$` and `${...}` handlers**

In `src/lexer.rs`, find `read_dollar_expansion` (the function called from the `$` arm). It currently dispatches on the character after `$`: a letter → variable name; `(` → command sub or arith; `{` → braced form; `?` → `LastStatus`; etc.

Add new branches for `$@`, `$*`, `$#`, and `$<digit>` — in the function's main match-on-next-char block, alongside the existing branches. The `quoted` flag is passed in from the caller (`true` inside double quotes). Example shape:

```rust
        Some('@') => {
            chars.next();
            parts.push(WordPart::AllArgs { joined: false, quoted });
            Ok(())
        }
        Some('*') => {
            chars.next();
            parts.push(WordPart::AllArgs { joined: true, quoted });
            Ok(())
        }
        Some('#') => {
            chars.next();
            parts.push(WordPart::Var { name: "#".to_string(), quoted });
            Ok(())
        }
        Some(c) if c.is_ascii_digit() => {
            let d = chars.next().unwrap();
            parts.push(WordPart::Var { name: d.to_string(), quoted });
            Ok(())
        }
```

In the braced-form parser (`${...}` — find the function that reads the name between `{` and `}`/`:` etc.), allow:

- A digit-only name (`${10}`, `${42}`) → `WordPart::Var { name, quoted }`.
- Special name `@` → `WordPart::AllArgs { joined: false, quoted }`.
- Special name `*` → `WordPart::AllArgs { joined: true, quoted }`.
- Special name `#` → `WordPart::Var { name: "#", quoted }`.

The existing name-validation in `${...}` likely rejects digit-leading names; relax it to accept digit-only sequences AND the special single chars `@`/`*`/`#` (only these; no parameter-expansion modifiers on the special names for v22).

- [ ] **Step 5: Add `Shell::positional_args` and `lookup_var`**

In `src/shell_state.rs`, add a field to `Shell`:

```rust
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
    /// Current frame of positional parameters. Populated only by
    /// function calls (Task 5); empty at the top level.
    pub positional_args: Vec<String>,
    #[allow(dead_code)]
    pub jobs: JobTable,
    pub sigchld_flag: Arc<AtomicBool>,
    pub sigint_flag: Arc<AtomicBool>,
    pub shell_pgid: i32,
    pub history: crate::history::History,
}
```

Initialise `positional_args: Vec::new()` in `Shell::new()`.

Add a `lookup_var` method:

```rust
/// Variable lookup for expansion. Recognises positional names
/// (`"1"`-`"9"`/`"10"`/..., and `"#"`) before falling back to the
/// regular variable HashMap. Returns an owned `String` because
/// positional/computed values are not stored as references.
pub fn lookup_var(&self, name: &str) -> Option<String> {
    if name == "#" {
        return Some(self.positional_args.len().to_string());
    }
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_digit()) {
        let n: usize = name.parse().ok()?;
        if n == 0 {
            return None; // $0 deferred
        }
        return self.positional_args.get(n - 1).cloned();
    }
    self.vars.get(name).map(|v| v.value.clone())
}
```

The existing `Shell::get(&str) -> Option<&str>` stays untouched for non-expander callers.

- [ ] **Step 6: Wire `lookup_var` into the expander**

In `src/expand.rs`, the `expand` function's `WordPart::Var` arms (there are two — one for quoted, one for unquoted) currently call `shell.get(name)`. Change them to `shell.lookup_var(name)` (which returns `Option<String>` — adjust the binding accordingly):

```rust
            WordPart::Var { name, quoted: true } => {
                if let Some(value) = shell.lookup_var(name) {
                    current.push_str(&value, true);
                    has_emitted = true;
                }
            }
            WordPart::Var { name, quoted: false } => {
                let value = shell.lookup_var(name).unwrap_or_default();
                emit_split_fields(&value, &mut current, &mut result, &mut has_emitted);
            }
```

In `expand_assignment` (the no-split version), do the same change to its `WordPart::Var` arm:

```rust
            WordPart::Var { name, .. } => {
                if let Some(value) = shell.lookup_var(name) {
                    result.push_str(&value);
                }
            }
```

`expand_pattern` (v21, in the same file) calls `expand_assignment` per part and so will pick up the change transparently — no edit needed there for `Var`.

- [ ] **Step 7: Add `WordPart::AllArgs` arms to the three expanders**

In `src/expand.rs`'s `expand` function, add a new arm. The semantics (Section 4 of the spec):

- Unquoted `$@`/`$*`: emit each arg as its own field, then each field is IFS-split (use the existing `emit_split_fields` helper that handles this for `Var { quoted: false }`).
- Quoted `"$@"` (joined=false): emit each arg as its own field with no splitting. The first arg merges into the current field (so `foo"$@"` becomes `fooarg1`, then `arg2`, …); subsequent args start new fields; the last arg merges into the current field again (so `"$@"bar` keeps `bar` attached to the last arg).
- Quoted `"$*"` (joined=true): all args concatenated into the current field, separated by the first IFS char (space).
- Empty `positional_args`: `$@`/`"$@"` produce zero fields (no emit, no field-break — match how an empty quoted Var behaves today); `$*` (joined unquoted) produces zero fields; `"$*"` produces an empty field (matches `""`).

Concrete arm sketch (adapt to the existing `current`/`result`/`has_emitted` machinery in `expand`):

```rust
            WordPart::AllArgs { quoted: false, joined: _ } => {
                // Unquoted $@ and $* behave identically: each positional
                // becomes its own field, then each is IFS-split.
                for arg in &shell.positional_args.clone() {
                    emit_split_fields(arg, &mut current, &mut result, &mut has_emitted);
                }
            }
            WordPart::AllArgs { quoted: true, joined: false } => {
                // "$@" — each arg becomes its own quoted field, no splitting.
                // The first arg merges into the current field; subsequent
                // ones start new fields; the last arg merges back so any
                // following text in the word attaches to it.
                let args = shell.positional_args.clone();
                let n = args.len();
                for (i, arg) in args.iter().enumerate() {
                    if i == 0 {
                        current.push_str(arg, true);
                        has_emitted = true;
                    } else {
                        // start a new field
                        result.push(std::mem::take(&mut current));
                        current.push_str(arg, true);
                        has_emitted = true;
                    }
                    let _ = n; // (no special end-of-loop merge; subsequent
                               // parts in the word naturally append to the
                               // current field — which is the last arg)
                }
            }
            WordPart::AllArgs { quoted: true, joined: true } => {
                // "$*" — single field, args joined by first IFS char (space).
                let joined = shell.positional_args.join(" ");
                current.push_str(&joined, true);
                has_emitted = true;
            }
```

If the existing `Field`/`current`/`result` types use different methods than `push_str(s, quoted)` / `std::mem::take`, adapt to whatever the file's existing `Var { quoted: true }` arm does for "emit one quoted field." The implementer reads the surrounding code and matches its idioms.

In `expand_assignment` (no-split context), add:

```rust
            WordPart::AllArgs { joined: _, .. } => {
                // No field splitting in assignment context; concatenate
                // with a space separator (matches POSIX behaviour for $*
                // in unquoted-but-no-split contexts).
                let joined = shell.positional_args.join(" ");
                result.push_str(&joined);
            }
```

In `expand_pattern` (v21, quote-aware glob builder), the per-part loop already delegates to `expand_assignment(&Word(vec![part.clone()]), shell)` for each part — `AllArgs` is handled automatically by the `expand_assignment` arm above. But verify: `WordPart::AllArgs` clones cleanly (no inner non-`Clone` types — it's just two bools).

Update `word_part_is_quoted` (in `src/expand.rs`, from v21) with the new variant:

```rust
fn word_part_is_quoted(part: &WordPart) -> bool {
    match part {
        WordPart::Literal { quoted, .. } => *quoted,
        WordPart::Var { quoted, .. } => *quoted,
        WordPart::LastStatus { quoted } => *quoted,
        WordPart::CommandSub { quoted, .. } => *quoted,
        WordPart::Arith { quoted, .. } => *quoted,
        WordPart::ParamExpansion { quoted, .. } => *quoted,
        WordPart::AllArgs { quoted, .. } => *quoted,
        WordPart::Tilde(_) => false,
    }
}
```

- [ ] **Step 8: Write the failing expander tests**

In `src/expand.rs`'s `#[cfg(test)] mod tests`, add (the existing module has helpers; adapt to its style — `Shell::new()`, manipulating `positional_args` directly):

```rust
#[test]
fn expand_dollar_digit_reads_positional() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["alpha".to_string(), "beta".to_string()];
    let w = Word(vec![WordPart::Var { name: "1".to_string(), quoted: false }]);
    let fields = expand(&w, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "alpha");
}

#[test]
fn expand_dollar_digit_unset_is_empty() {
    let mut shell = Shell::new();
    let w = Word(vec![WordPart::Var { name: "1".to_string(), quoted: false }]);
    let fields = expand(&w, &mut shell);
    // Unset positional → no field (consistent with unset var behaviour)
    assert!(fields.iter().all(|f| f.chars.is_empty()));
}

#[test]
fn expand_dollar_hash_is_arg_count() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let w = Word(vec![WordPart::Var { name: "#".to_string(), quoted: false }]);
    let fields = expand(&w, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "3");
}

#[test]
fn expand_dollar_at_quoted_produces_field_per_arg() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a a".to_string(), "b".to_string()];
    let w = Word(vec![WordPart::AllArgs { joined: false, quoted: true }]);
    let fields = expand(&w, &mut shell);
    // Each arg is its own field; the spaces inside "a a" are preserved.
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].chars, "a a");
    assert_eq!(fields[1].chars, "b");
}

#[test]
fn expand_dollar_star_quoted_joins_with_space() {
    let mut shell = Shell::new();
    shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    let w = Word(vec![WordPart::AllArgs { joined: true, quoted: true }]);
    let fields = expand(&w, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "a b c");
}

#[test]
fn expand_dollar_at_empty_produces_no_fields() {
    let mut shell = Shell::new();
    let w = Word(vec![WordPart::AllArgs { joined: false, quoted: true }]);
    let fields = expand(&w, &mut shell);
    assert!(fields.is_empty() || fields.iter().all(|f| f.chars.is_empty()));
}
```

- [ ] **Step 9: Run the tests**

Run: `cargo test`
Expected: PASS — the 7 lexer tests, 6 expander tests, and all prior 814 + Task 1's + Task 2's + Task 3's all pass.

- [ ] **Step 10: Commit**

```bash
git add src/lexer.rs src/expand.rs src/shell_state.rs
git commit -m "v22 task 4: positional parameters ($1, $@, $*, $#, ${N})"
```

---

## Task 5: Function call mechanism

**Files:**
- Modify: `src/shell_state.rs` (`functions` field)
- Modify: `src/executor.rs` (FunctionDef execution; function-call dispatch)

Wires together Tasks 2-4: the function table on `Shell`, real execution of `Command::FunctionDef` (registers the function), and the dispatch path in `run_exec_single` that finds a user-defined function by name and calls it with the right positional-arg frame.

- [ ] **Step 1: Add the function table to `Shell`**

In `src/shell_state.rs`, add a field to `Shell`:

```rust
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
    pub positional_args: Vec<String>,
    /// User-defined functions. Populated by `Command::FunctionDef`
    /// execution; looked up by `run_exec_single` when dispatching a
    /// simple command.
    pub functions: HashMap<String, Box<crate::command::Command>>,
    #[allow(dead_code)]
    pub jobs: JobTable,
    pub sigchld_flag: Arc<AtomicBool>,
    pub sigint_flag: Arc<AtomicBool>,
    pub shell_pgid: i32,
    pub history: crate::history::History,
}
```

Initialise `functions: HashMap::new()` in `Shell::new()`.

- [ ] **Step 2: Write the failing executor test**

In `src/executor.rs`'s `#[cfg(test)] mod tests`, add an in-process unit test for the FunctionDef-execution path (the function-call dispatch path is exercised end-to-end by Task 6's integration tests — `CARGO_BIN_EXE_huck` is only available to integration tests, not unit tests, so the call-mechanism tests live in Task 6):

```rust
#[test]
fn function_def_registers_and_returns_zero() {
    let body = Sequence {
        first: Command::Pipeline(Pipeline {
            commands: vec![SimpleCommand::Exec(ExecCommand {
                program: Word(vec![WordPart::Literal { text: "echo".into(), quoted: false }]),
                args: vec![Word(vec![WordPart::Literal { text: "hi".into(), quoted: false }])],
                stdin: None, stdout: None, stderr: None,
            })],
        }),
        rest: vec![],
        background: false,
    };
    let def = Sequence {
        first: Command::FunctionDef {
            name: "f".to_string(),
            body: Box::new(Command::BraceGroup(Box::new(body))),
        },
        rest: vec![],
        background: false,
    };
    let mut shell = Shell::new();
    let (_, status) = execute_capturing(&def, &mut shell);
    assert_eq!(status, 0);
    assert!(shell.functions.contains_key("f"));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --lib function_def_registers_and_returns_zero`
Expected: FAIL — `run_command`'s `Command::FunctionDef { .. }` arm is the `unreachable!` placeholder from Task 3.

- [ ] **Step 4: Implement `FunctionDef` execution**

In `src/executor.rs`, replace the placeholder arm in `run_command`:

```rust
        Command::FunctionDef { name, body } => {
            shell.functions.insert(name.clone(), body.clone());
            ExecOutcome::Continue(0)
        }
```

- [ ] **Step 5: Add `call_function` and wire dispatch in `run_exec_single`**

In `src/executor.rs`, add a helper near the other `run_*` functions:

```rust
/// Runs a function body in a new positional-arg frame. The args slice
/// is the call's arguments *excluding* the function name — POSIX `$1`
/// is the first user arg. Catches `FunctionReturn`; `Exit`/`LoopBreak`/
/// `LoopContinue` propagate unchanged so `break` inside a function
/// targets the caller's enclosing loop (matching bash).
fn call_function(
    body: Box<crate::command::Command>,
    args: Vec<String>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    let saved = std::mem::take(&mut shell.positional_args);
    shell.positional_args = args;
    let result = run_command(&body, shell, sink);
    shell.positional_args = saved;
    match result {
        ExecOutcome::FunctionReturn(n) => ExecOutcome::Continue(n),
        other => other,
    }
}

fn is_control_builtin(name: &str) -> bool {
    matches!(name, "return" | "exit" | "break" | "continue")
}
```

In `src/executor.rs`, find `run_exec_single`. It currently calls `resolve(cmd, shell)` then dispatches the resolved program/args to either a builtin (`run_builtin`) or a forked exec. The dispatch ordering is what changes — between control-builtin and regular-builtin checks, interject the function-table lookup:

```rust
fn run_exec_single(exec: &ExecCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let resolved = match resolve(exec, shell) {
        Ok(r) => r,
        Err(code) => return ExecOutcome::Continue(code),
    };
    let program = &resolved.program;
    // 1. Control builtins always win.
    if is_control_builtin(program) {
        return run_with_redirects(&resolved, shell, sink); // or whatever the existing path is
    }
    // 2. User-defined function?
    if let Some(body) = shell.functions.get(program).cloned() {
        return call_function(body, resolved.args.clone(), shell, sink);
    }
    // 3. Regular builtin?
    if crate::builtins::is_builtin(program) {
        return run_with_redirects(&resolved, shell, sink);
    }
    // 4. PATH-exec.
    run_external(&resolved, shell, sink)
}
```

The exact existing structure of `run_exec_single` will be different — adapt the dispatch ordering to the established pattern in the file. The key invariant is: control-builtin → function → other-builtin → exec.

Note: `shell.functions.get(program).cloned()` clones the `Box<Command>` — needed because we then run the body while `shell` is borrowed mutably elsewhere. `Command` derives `Clone`, so this works; the clone is per-call and acceptable for a learning shell.

- [ ] **Step 6: Run the tests**

Run: `cargo test`
Expected: PASS — the 1 new executor test passes and all prior tests still pass. (The end-to-end function-call behaviour is verified by Task 6's integration suite.)

- [ ] **Step 7: Manual smoke test**

```bash
cargo build --release
./target/release/huck <<'EOF'
greet() { echo "hello, $1"; }
greet world
add() { echo $(($1 + $2)); }
add 3 4
showall() { echo "n=$#"; for x in "$@"; do echo "  arg=$x"; done; }
showall a "b c" d
countdown() { if test $1 -le 0; then echo done; return 0; fi; echo $1; countdown $(( $1 - 1 )); }
countdown 3
f() { for x in 1 2 3 4; do if test $x = 2; then break; fi; echo iter-$x; done; }
f
exit
EOF
```

Expected output, in order: `hello, world`; `7`; `n=3`, `  arg=a`, `  arg=b c`, `  arg=d`; `3`, `2`, `1`, `done`; `iter-1`. Report the actual output verbatim.

- [ ] **Step 8: Commit**

```bash
git add src/shell_state.rs src/executor.rs
git commit -m "v22 task 5: function call mechanism (table, dispatch, scope)"
```

---

## Task 6: End-to-end integration tests

**Files:**
- Create: `tests/functions_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/functions_integration.rs`:

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
fn basic_function_definition_and_call() {
    let (out, _) = run("f() { echo hi; }\nf\nexit\n");
    assert!(out.lines().any(|l| l == "hi"), "stdout: {out}");
}

#[test]
fn function_with_positional_args() {
    let (out, _) = run("add() { echo $(($1 + $2)); }\nadd 3 4\nexit\n");
    assert!(out.lines().any(|l| l == "7"), "stdout: {out}");
}

#[test]
fn dollar_hash_is_argument_count() {
    let (out, _) = run("f() { echo n=$#; }\nf a b c d\nexit\n");
    assert!(out.lines().any(|l| l == "n=4"), "stdout: {out}");
}

#[test]
fn dollar_at_unquoted_word_splits() {
    let (out, _) = run("f() { for x in $@; do echo i-$x; done; }\nf alpha beta gamma\nexit\n");
    assert!(out.lines().any(|l| l == "i-alpha"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "i-beta"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "i-gamma"), "stdout: {out}");
}

#[test]
fn dollar_at_quoted_preserves_args() {
    // "$@" preserves each arg as its own field, even if it contains spaces.
    let (out, _) = run(
        "f() { for x in \"$@\"; do echo i=$x; done; }\nf \"hello world\" foo\nexit\n",
    );
    assert!(out.lines().any(|l| l == "i=hello world"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "i=foo"), "stdout: {out}");
}

#[test]
fn dollar_star_quoted_joins() {
    let (out, _) = run("f() { echo \"all=$*\"; }\nf a b c\nexit\n");
    assert!(out.lines().any(|l| l == "all=a b c"), "stdout: {out}");
}

#[test]
fn return_with_status() {
    let (out, _) = run("f() { return 7; }\nf\necho status-$?\nexit\n");
    assert!(out.lines().any(|l| l == "status-7"), "stdout: {out}");
}

#[test]
fn return_exits_early() {
    let (out, _) = run(
        "f() { echo before; return; echo never; }\nf\necho after\nexit\n",
    );
    assert!(out.lines().any(|l| l == "before"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "never"), "return failed: {out}");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
}

#[test]
fn function_in_and_or_sequence() {
    let (out, _) = run("f() { return 0; }\nf && echo yes\nexit\n");
    assert!(out.lines().any(|l| l == "yes"), "stdout: {out}");
}

#[test]
fn function_recursion() {
    let script = "countdown() { if test $1 -le 0; then echo done; return; fi; echo $1; countdown $(( $1 - 1 )); }\ncountdown 3\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "3"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "2"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "done"), "stdout: {out}");
}

#[test]
fn function_shadows_regular_builtin() {
    let (out, _) = run("echo() { :; }\necho should-be-silenced\nexit\n");
    // The body `:` doesn't exist in huck — but the function still runs and
    // suppresses the literal output via the override.
    // (If `:` produces an error, that's fine — the key assertion is that
    //  the literal "should-be-silenced" does NOT appear in stdout.)
    assert!(!out.lines().any(|l| l == "should-be-silenced"), "stdout: {out}");
}

#[test]
fn return_is_unshadowable() {
    let (out, _) = run(
        "return() { echo BAD; }\nf() { return 3; echo NEVER; }\nf\necho status-$?\nexit\n",
    );
    assert!(!out.lines().any(|l| l == "BAD"), "return called the user fn: {out}");
    assert!(!out.lines().any(|l| l == "NEVER"), "return did not exit f: {out}");
    assert!(out.lines().any(|l| l == "status-3"), "stdout: {out}");
}

#[test]
fn break_inside_function_targets_callers_loop() {
    // POSIX/bash: `break` inside a function affects the caller's loop.
    let script = "leave() { break; }\nfor x in a b c; do echo at-$x; leave; done\necho after\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "at-a"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "at-b"), "break did not exit caller's loop: {out}");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
}

#[test]
fn multiline_function_definition() {
    let script = "f() {\n  echo line1\n  echo line2\n}\nf\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "line1"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "line2"), "stdout: {out}");
}

#[test]
fn standalone_brace_group_runs_in_current_shell() {
    let (out, _) = run("{ x=brace; echo x-set; }\necho after-x=$x\nexit\n");
    assert!(out.lines().any(|l| l == "x-set"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "after-x=brace"), "no isolation: {out}");
}

#[test]
fn stray_return_at_top_level_is_harmless() {
    let (out, _) = run("return\necho still-alive\nexit\n");
    assert!(out.lines().any(|l| l == "still-alive"), "stdout: {out}");
}

#[test]
fn function_body_can_be_if() {
    let script = "test_arg() if test $1 = yes; then echo matched; else echo other; fi\ntest_arg yes\ntest_arg no\nexit\n";
    let (out, _) = run(script);
    assert!(out.lines().any(|l| l == "matched"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "other"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test functions_integration`
Expected: 17 tests pass.

If a test fails, inspect actual stdout/stderr. Behaviour was implemented in Tasks 1-5; only adjust an assertion if it genuinely mismatches *correct* shell output and you can explain why. Never change `src/` to make a test pass.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: PASS — all tests, prior plus the 17 new.

- [ ] **Step 4: Commit**

```bash
git add tests/functions_integration.rs
git commit -m "v22 task 6: end-to-end function integration tests"
```

---

## Task 7: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v22 row to the status table**

Append after the v21 row, matching the table's column alignment:

```
| v22       | Functions (`name() { … }`) + positional parameters      |
```

- [ ] **Step 2: Add a feature note**

After the v21 `case`-statements block, add:

```markdown
**Functions (v22):**
`name() compound-command` defines a function (the canonical body is a
brace group `{ … }`, but any compound — `if`/`while`/`for`/`case`/
`{ … }` — works). Calling `name arg1 arg2 …` runs the body with the
positional parameters `$1`, `$2`, … set to the call's arguments and
restored afterward. `$@` and `$*` give all args (`"$@"` preserves each
as its own field — the only construct that produces multiple fields
when quoted; `"$*"` joins them with a space). `$#` is the argument
count. `return [N]` exits a function early with status `N` (defaulting
to `$?`). A function shadows any builtin except the flow-control set
(`return`/`exit`/`break`/`continue`), so `cd() { … }` works but
`return() { … }` is unreachable. `break`/`continue` inside a function
target the caller's enclosing loop (matching bash). `local` variable
scoping, `set --` / `shift`, and `$0` are not implemented. v22 also
adds the standalone brace group `{ list; }` (runs in the current shell
— no subshell isolation).
```

- [ ] **Step 3: Update the "Not yet implemented" section**

The README's "Not yet implemented:" paragraph lists `functions` among the missing features. Find it and remove `functions`. If it reads `... functions, here-docs, aliases.` it becomes `... here-docs, aliases.`. Read the actual wording and edit precisely.

- [ ] **Step 4: Update the test-suite count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the `passed` counts across all lines. The README's Build-and-run section has a line `cargo test               # full test suite (NNN tests)`. Update `NNN`. Expected ≈ 860 — use the actual number.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "v22 task 7: README — functions and positional parameters"
```

---

## Final review checkpoint

After Task 7:

- [ ] `cargo test` shows the expected total passing, 0 failing.
- [ ] `cargo clippy --all-targets -- -D warnings` introduces no new lints versus `main` (the codebase has ~34 lints after the v21-followup cleanup; only new ones count).
- [ ] Manual REPL smoke session: a basic function `f() { echo hi; }; f`; positional args `add() { echo $(($1+$2)); }; add 3 4`; `$@` quoted preserving args with spaces; `$#`; `return` with a status; a recursive function; a function shadowing `cd`; `return` inside a `while` exiting only the function; `break` inside a function exiting the caller's `while`; a standalone `{ a=x; echo $a; }`; a stray `return` at the top level being harmless; an `if`/`for`/`case` as a function body; a multi-line `f() { ... }` definition.
- [ ] Confirm `if`/`while`/`for`/`case` and all previously-shipped behaviour still work. Confirm a quoted `{`/`}` is still a literal word.
- [ ] Final-review the whole branch as a single diff before merging to `main`.
