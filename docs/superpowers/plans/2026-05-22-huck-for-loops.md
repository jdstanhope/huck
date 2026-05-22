# huck v20: `for` Loops Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the POSIX `for NAME in WORD...; do LIST; done` loop to huck.

**Architecture:** `for` is a compound command following the v17/v18 pattern: a new `Command::For` AST variant, a recursive-descent `parse_for`, and an executor `run_for` that mirrors `run_while`. The word list reuses the existing argument-expansion path (`expand` + `glob_expand_fields`), expanded once before the loop. `break`/`continue` (v18) and multi-line input (v19) work with no new machinery.

**Tech Stack:** Rust 2024, `tempfile` (dev-dependency, used by one integration test).

---

## File Map

| File | Change | Responsibility |
| --- | --- | --- |
| `src/command.rs` | Modify | `Command::For`, `ForClause`, `Keyword::{For,In}`, `ParseError::ForVariable`, `parse_for` |
| `src/executor.rs` | Modify | `run_for`; the `run_command` dispatch arm |
| `src/shell.rs` | Modify | `parse_error_message` arm for `ForVariable` |
| `tests/for_integration.rs` | Create | End-to-end piped `for`-loop scripts |
| `README.md` | Modify | v20 status row and feature note |

**Baseline:** `cargo test` passes 733 tests before this work begins.

---

## Task 1: AST, keywords, and parser

**Files:**
- Modify: `src/command.rs`
- Modify: `src/executor.rs` (placeholder `run_command` arm — the real executor is Task 2)
- Modify: `src/shell.rs` (`parse_error_message` arm)

Adding the `Command::For` variant makes several `match`es non-exhaustive, so the AST addition, the parser, the placeholder executor arm, and the test-helper arms must all land together to compile.

- [ ] **Step 1: Write the failing parser tests**

In `src/command.rs`, inside `#[cfg(test)] mod tests`, add a `first_for` helper next to `first_while`, and the tests. The helpers `w_tok`, `kw`, `plain` already exist; `kw` is an alias of `w_tok`.

```rust
/// Extracts the ForClause from a sequence whose first command is a For.
fn first_for(seq: &Sequence) -> &ForClause {
    match &seq.first {
        Command::For(c) => c,
        other => panic!("expected a for, got {other:?}"),
    }
}

#[test]
fn parse_simple_for() {
    // for x in a b c ; do echo ; done
    let seq = parse(vec![
        kw("for"), w_tok("x"), kw("in"),
        w_tok("a"), w_tok("b"), w_tok("c"), Token::Op(Operator::Semi),
        kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    let clause = first_for(&seq);
    assert_eq!(clause.var, "x");
    assert_eq!(clause.words.len(), 3);
    assert_eq!(
        clause.body.first,
        Command::Pipeline(Pipeline { commands: vec![plain("echo", &[])] })
    );
}

#[test]
fn parse_for_multiline_matches_singleline() {
    // newlines in place of the `;` separators parse identically
    let multiline = parse(vec![
        kw("for"), w_tok("x"), kw("in"), w_tok("a"), Token::Newline,
        kw("do"), w_tok("echo"), Token::Newline,
        kw("done"),
    ]).unwrap().unwrap();
    let singleline = parse(vec![
        kw("for"), w_tok("x"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert_eq!(multiline, singleline);
}

#[test]
fn parse_for_no_in_has_empty_words() {
    // for x ; do echo ; done  — no `in`, zero iterations
    let seq = parse(vec![
        kw("for"), w_tok("x"), Token::Op(Operator::Semi),
        kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert!(first_for(&seq).words.is_empty());
}

#[test]
fn parse_for_empty_in_list() {
    // for x in ; do echo ; done  — `in` present, list empty
    let seq = parse(vec![
        kw("for"), w_tok("x"), kw("in"), Token::Op(Operator::Semi),
        kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert!(first_for(&seq).words.is_empty());
}

#[test]
fn parse_for_do_terminates_word_list() {
    // for x in a b do echo ; done  — `do` ends the list, no `;`
    let seq = parse(vec![
        kw("for"), w_tok("x"), kw("in"), w_tok("a"), w_tok("b"),
        kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert_eq!(first_for(&seq).words.len(), 2);
}

#[test]
fn parse_for_keyword_words_in_list() {
    // for x in then else ; do echo ; done  — keywords are ordinary values
    let seq = parse(vec![
        kw("for"), w_tok("x"), kw("in"), w_tok("then"), w_tok("else"),
        Token::Op(Operator::Semi),
        kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert_eq!(first_for(&seq).words.len(), 2);
}

#[test]
fn parse_for_in_on_next_line() {
    // for x <NL> in a <NL> do echo <NL> done
    let seq = parse(vec![
        kw("for"), w_tok("x"), Token::Newline,
        kw("in"), w_tok("a"), Token::Newline,
        kw("do"), w_tok("echo"), Token::Newline,
        kw("done"),
    ]).unwrap().unwrap();
    let clause = first_for(&seq);
    assert_eq!(clause.var, "x");
    assert_eq!(clause.words.len(), 1);
}

#[test]
fn parse_for_invalid_variable_name_errors() {
    // a leading digit is not a valid identifier
    assert_eq!(
        parse(vec![
            kw("for"), w_tok("2x"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
        ]),
        Err(ParseError::ForVariable)
    );
}

#[test]
fn parse_for_keyword_as_variable_errors() {
    // `in` is a reserved word and cannot be the loop variable
    assert_eq!(
        parse(vec![
            kw("for"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
        ]),
        Err(ParseError::ForVariable)
    );
}

#[test]
fn parse_for_unterminated_is_unterminated_loop() {
    // `for x in a` with no `do`/`done` — incomplete, not a hard error
    assert_eq!(
        parse(vec![kw("for"), w_tok("x"), kw("in"), w_tok("a")]),
        Err(ParseError::UnterminatedLoop)
    );
    // bare `for` at end of input
    assert_eq!(parse(vec![kw("for")]), Err(ParseError::UnterminatedLoop));
}

#[test]
fn parse_for_operator_in_word_list_errors() {
    // for x in a | b ; do ...  — `|` is not a valid list element
    assert_eq!(
        parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("a"),
            Token::Op(Operator::Pipe), w_tok("b"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
        ]),
        Err(ParseError::UnexpectedToken)
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib parse_simple_for`
Expected: FAIL — compile error: `Command::For`, `ForClause`, `ParseError::ForVariable`, `first_for` do not exist.

- [ ] **Step 3: Add the keywords**

In `src/command.rs`, extend the `Keyword` enum — add `For` and `In` after `Done`:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Keyword {
    If,
    Then,
    Elif,
    Else,
    Fi,
    While,
    Until,
    Do,
    Done,
    For,
    In,
}
```

In `Keyword::name`, add the two arms:

```rust
            Keyword::Do => "do",
            Keyword::Done => "done",
            Keyword::For => "for",
            Keyword::In => "in",
```

In `keyword_of`, add the two mappings:

```rust
        "do" => Some(Keyword::Do),
        "done" => Some(Keyword::Done),
        "for" => Some(Keyword::For),
        "in" => Some(Keyword::In),
```

- [ ] **Step 4: Add the AST**

In `src/command.rs`, extend the `Command` enum:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
    While(Box<WhileClause>),
    For(Box<ForClause>),
}
```

Add the `ForClause` struct right after `WhileClause`:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ForClause {
    /// The loop variable name — a validated identifier.
    pub var: String,
    /// The unexpanded `in` word list. Empty for the no-`in` form.
    pub words: Vec<Word>,
    /// The do…done body.
    pub body: Sequence,
}
```

Add the `ForVariable` variant to `ParseError`:

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
    ForVariable,
}
```

- [ ] **Step 5: Add `parse_for` and the `for_variable_name` helper**

In `src/command.rs`, add these two functions next to `parse_while`:

```rust
/// Returns the loop-variable name if `token` is a single, unquoted
/// `Literal` `Word` whose text is a valid identifier and not a reserved
/// keyword. Otherwise `None`.
fn for_variable_name(token: &Token) -> Option<String> {
    if keyword_of(token).is_some() {
        return None;
    }
    let Token::Word(Word(parts)) = token else {
        return None;
    };
    if parts.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &parts[0] else {
        return None;
    };
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

/// Parses `for NAME [in WORD...] sep do LIST done`. The caller has
/// peeked the leading `for`.
fn parse_for<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<ForClause, ParseError> {
    expect_keyword(iter, Keyword::For, ParseError::UnterminatedLoop)?;

    // Loop variable. End-of-input means the command is incomplete (the
    // v19 classifier maps UnterminatedLoop to "read more"); a present
    // but invalid token is a genuine error.
    let var = match iter.next() {
        None => return Err(ParseError::UnterminatedLoop),
        Some(tok) => for_variable_name(&tok).ok_or(ParseError::ForVariable)?,
    };

    // POSIX allows a linebreak between the variable and `in`.
    skip_newlines(iter);

    // Optional `in` plus the word list.
    let mut words: Vec<Word> = Vec::new();
    if iter.peek().and_then(keyword_of) == Some(Keyword::In) {
        iter.next(); // consume `in`
        loop {
            match iter.peek() {
                None | Some(Token::Newline) | Some(Token::Op(Operator::Semi)) => break,
                Some(tok) => {
                    if keyword_of(tok) == Some(Keyword::Do) {
                        break;
                    }
                    match iter.next() {
                        Some(Token::Word(w)) => words.push(w),
                        Some(Token::Op(_)) => return Err(ParseError::UnexpectedToken),
                        _ => unreachable!("peek already ruled out Newline/Semi/None here"),
                    }
                }
            }
        }
    }

    // Skip `;`/newline separators, then `do`.
    while matches!(
        iter.peek(),
        Some(Token::Op(Operator::Semi)) | Some(Token::Newline)
    ) {
        iter.next();
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;

    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(ForClause { var, words, body })
}
```

- [ ] **Step 6: Wire `parse_for` into `parse_command`**

In `src/command.rs`, `parse_command`'s `match` currently has arms for `If`, `While`/`Until`, `other`, `None`. Add a `For` arm:

```rust
    match iter.peek().and_then(keyword_of) {
        Some(Keyword::If) => Ok(Command::If(Box::new(parse_if(iter)?))),
        Some(Keyword::While) | Some(Keyword::Until) => {
            Ok(Command::While(Box::new(parse_while(iter)?)))
        }
        Some(Keyword::For) => Ok(Command::For(Box::new(parse_for(iter)?))),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => Ok(Command::Pipeline(parse_pipeline(iter)?)),
    }
```

- [ ] **Step 7: Add the test-helper arms for the new `Command` variant**

Adding `Command::For` makes two existing test helpers' `match`es non-exhaustive. In `src/command.rs`'s test module:

`first_pipeline` — add a `For` arm:

```rust
    fn first_pipeline(seq: &Sequence) -> &Pipeline {
        match &seq.first {
            Command::Pipeline(p) => p,
            Command::If(_) => panic!("expected a pipeline, got an if"),
            Command::While(_) => panic!("expected a pipeline, got a while"),
            Command::For(_) => panic!("expected a pipeline, got a for"),
        }
    }
```

`first_if` — add a `For` arm:

```rust
    fn first_if(seq: &Sequence) -> &IfClause {
        match &seq.first {
            Command::If(c) => c,
            Command::Pipeline(_) => panic!("expected an if, got a pipeline"),
            Command::While(_) => panic!("expected an if, got a while"),
            Command::For(_) => panic!("expected an if, got a for"),
        }
    }
```

- [ ] **Step 8: Add the placeholder executor arm**

In `src/executor.rs`, `run_command` currently matches `Pipeline`/`If`/`While`. Add a placeholder `For` arm (the real `run_for` is Task 2):

```rust
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink),
        Command::If(clause) => run_if(clause, shell, sink),
        Command::While(clause) => run_while(clause, shell, sink),
        Command::For(_) => unreachable!("run_for lands in v20 task 2"),
    }
}
```

- [ ] **Step 9: Add the `ForVariable` message in `shell.rs`**

In `src/shell.rs`, `parse_error_message` matches over `ParseError`. Add an arm (the message carries no leading colon — the caller supplies `": "`):

```rust
        ParseError::ForVariable => "invalid variable name in 'for' loop".to_string(),
```

- [ ] **Step 10: Build and run the tests**

Run: `cargo test`
Expected: PASS — the 11 new parser tests pass and all 733 prior tests still pass. If the compiler reports a non-exhaustive `match` on `Command` at any site other than `run_command`, `first_pipeline`, `first_if`, add a minimal correct arm and note it in the report.

- [ ] **Step 11: Commit**

```bash
git add src/command.rs src/executor.rs src/shell.rs
git commit -m "v20 task 1: for-loop AST, keywords, and parser"
```

---

## Task 2: Executor — `run_for`

**Files:**
- Modify: `src/executor.rs`

- [ ] **Step 1: Write the failing executor tests**

`src/executor.rs`'s `#[cfg(test)] mod tests` has helpers from v17/v18: `seq_of`, `echo_seq`, `cond_seq`, `lit_word`, `while_seq`, `break_seq`, `execute_capturing`. Confirm they exist; `lit_word(s: &str) -> Word` builds a single-`Literal` `Word`, `break_seq()` is a one-pipeline `Sequence` running the `break` builtin, `execute_capturing(&Sequence, &mut Shell) -> (String, i32)` runs a sequence capturing stdout.

Add to the test module — first the helpers:

```rust
use crate::command::ForClause;

/// A Sequence wrapping a single `for` clause.
fn for_seq(clause: ForClause) -> Sequence {
    Sequence { first: Command::For(Box::new(clause)), rest: vec![], background: false }
}

/// A one-pipeline Sequence running `echo $<var>` (the variable expanded).
fn echo_var_seq(var: &str) -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline {
            commands: vec![SimpleCommand::Exec(ExecCommand {
                program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                args: vec![Word(vec![WordPart::Var { name: var.to_string(), quoted: false }])],
                stdin: None,
                stdout: None,
                stderr: None,
            })],
        }),
        rest: vec![],
        background: false,
    }
}

/// A one-pipeline Sequence running the `continue` builtin.
fn continue_seq() -> Sequence {
    Sequence {
        first: Command::Pipeline(Pipeline {
            commands: vec![SimpleCommand::Exec(ExecCommand {
                program: Word(vec![WordPart::Literal { text: "continue".to_string(), quoted: false }]),
                args: vec![],
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
fn for_iterates_each_value_in_order() {
    // for x in a b c; do echo $x; done
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
        body: echo_var_seq("x"),
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(out.lines().collect::<Vec<_>>(), vec!["a", "b", "c"]);
    assert_eq!(status, 0);
}

#[test]
fn for_empty_list_runs_body_zero_times() {
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![],
        body: echo_seq("hi"),
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn for_variable_holds_last_value_after_loop() {
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
        body: echo_var_seq("x"),
    };
    let mut shell = Shell::new();
    execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(shell.get("x"), Some("c"));
}

#[test]
fn for_break_stops_iteration() {
    // body breaks immediately — only the first value is ever assigned
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
        body: break_seq(),
    };
    let mut shell = Shell::new();
    let (_out, status) = execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(shell.get("x"), Some("a"));
    assert_eq!(status, 0);
}

#[test]
fn for_continue_advances_through_all_values() {
    let clause = ForClause {
        var: "x".to_string(),
        words: vec![lit_word("a"), lit_word("b"), lit_word("c")],
        body: continue_seq(),
    };
    let mut shell = Shell::new();
    execute_capturing(&for_seq(clause), &mut shell);
    assert_eq!(shell.get("x"), Some("c"));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib for_iterates_each_value_in_order`
Expected: FAIL — `run_command`'s `Command::For` arm is still the `unreachable!` placeholder, so it panics.

- [ ] **Step 3: Implement `run_for` and wire it in**

In `src/executor.rs`, add `ForClause` to the `use crate::command::{...}` import (the line that already imports `WhileClause`, `IfClause`, etc.).

Replace the `run_command` placeholder arm:

```rust
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink),
        Command::If(clause) => run_if(clause, shell, sink),
        Command::While(clause) => run_while(clause, shell, sink),
        Command::For(clause) => run_for(clause, shell, sink),
    }
}
```

Add `run_for` next to `run_while`:

```rust
/// Runs a `for` loop. The word list is expanded once, up front; the
/// body then runs with the loop variable set to each value in turn.
/// `break` ends the loop, `continue` advances to the next value,
/// `exit` propagates, and a pending SIGINT (Ctrl-C) ends the loop
/// with status 130.
fn run_for(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    use std::sync::atomic::Ordering;

    // Expand the word list once — the same path command arguments take.
    let mut values: Vec<String> = Vec::new();
    for word in &clause.words {
        values.extend(glob_expand_fields(expand(word, shell)));
    }

    let mut last = ExecOutcome::Continue(0);
    for value in values {
        if shell
            .sigint_flag
            .compare_exchange(true, false, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return ExecOutcome::Continue(130);
        }
        shell.set(&clause.var, value);
        match execute_sequence_body(&clause.body, shell, sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak => {
                last = ExecOutcome::Continue(0);
                break;
            }
            ExecOutcome::LoopContinue => {
                last = ExecOutcome::Continue(0);
                // fall through — advance to the next value
            }
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }
    }
    last
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test`
Expected: PASS — the 5 new executor tests plus all prior tests.

- [ ] **Step 5: Manual smoke test**

```bash
cargo build --release
./target/release/huck <<'EOF'
for x in alpha beta gamma; do echo val-$x; done
for i in 1 2 3
do
  echo line-$i
done
for x in a b c d; do if test $x = c; then break; fi; echo keep-$x; done
for x in a b c; do if test $x = b; then continue; fi; echo show-$x; done
for x in one two; do echo $x; done
echo after-loop-$x
for q in; do echo NEVER; done
echo empty-done
exit
EOF
```

Expected output, in order: `val-alpha`, `val-beta`, `val-gamma`; `line-1`, `line-2`, `line-3`; `keep-a`, `keep-b`; `show-a`, `show-c`; `one`, `two`; `after-loop-two`; `empty-done`. Report the actual output verbatim.

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs
git commit -m "v20 task 2: executor run_for — for-loop execution"
```

---

## Task 3: End-to-end integration tests

**Files:**
- Create: `tests/for_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/for_integration.rs`:

```rust
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Runs huck with `script` piped to stdin, in working directory `dir`.
/// Returns (stdout, stderr).
fn run_in_dir(dir: &Path, script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .current_dir(dir)
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

/// Runs huck with `script` piped to stdin, in the test process's cwd.
fn run(script: &str) -> (String, String) {
    run_in_dir(Path::new("."), script)
}

#[test]
fn for_over_literal_list() {
    let (out, _) = run("for x in a b c; do echo v-$x; done\nexit\n");
    let got: Vec<&str> = out.lines().filter(|l| l.starts_with("v-")).collect();
    assert_eq!(got, vec!["v-a", "v-b", "v-c"], "stdout: {out}");
}

#[test]
fn for_empty_list_runs_zero_times() {
    let (out, _) = run("for x in; do echo NOPE; done\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "NOPE"), "stdout: {out}");
}

#[test]
fn for_no_in_runs_zero_times() {
    let (out, _) = run("for x; do echo NOPE; done\necho after\nexit\n");
    assert!(out.lines().any(|l| l == "after"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "NOPE"), "stdout: {out}");
}

#[test]
fn for_over_command_substitution() {
    let (out, _) = run("for n in $(echo 1 2 3); do echo n$n; done\nexit\n");
    for marker in ["n1", "n2", "n3"] {
        assert!(out.lines().any(|l| l == marker), "missing {marker}: {out}");
    }
}

#[test]
fn for_word_splits_unquoted_variable() {
    let (out, _) = run("list=\"a b c\"\nfor x in $list; do echo i-$x; done\nexit\n");
    let got: Vec<&str> = out.lines().filter(|l| l.starts_with("i-")).collect();
    assert_eq!(got, vec!["i-a", "i-b", "i-c"], "stdout: {out}");
}

#[test]
fn for_over_glob() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();
    std::fs::write(dir.path().join("c.log"), "").unwrap();
    let (out, _) = run_in_dir(dir.path(), "for f in *.txt; do echo got-$f; done\nexit\n");
    assert!(out.lines().any(|l| l == "got-a.txt"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "got-b.txt"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "got-c.log"), "stdout: {out}");
}

#[test]
fn for_multiline() {
    let (out, _) = run("for x in a b\ndo\necho m-$x\ndone\nexit\n");
    assert!(out.lines().any(|l| l == "m-a"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "m-b"), "stdout: {out}");
}

#[test]
fn for_nested_inside_if() {
    let (out, _) = run("if true; then for x in a b; do echo f-$x; done; fi\nexit\n");
    assert!(out.lines().any(|l| l == "f-a"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "f-b"), "stdout: {out}");
}

#[test]
fn while_nested_inside_for() {
    let script = "for x in p q; do i=0; while test $i -lt 2; do echo $x$i; i=$((i+1)); done; done\nexit\n";
    let (out, _) = run(script);
    for marker in ["p0", "p1", "q0", "q1"] {
        assert!(out.lines().any(|l| l == marker), "missing {marker}: {out}");
    }
}

#[test]
fn for_break_exits_early() {
    let (out, _) = run(
        "for x in a b c d; do if test $x = c; then break; fi; echo k-$x; done\nexit\n",
    );
    assert!(out.lines().any(|l| l == "k-a"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "k-b"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "k-c"), "loop should have broken: {out}");
}

#[test]
fn for_continue_skips_iteration() {
    let (out, _) = run(
        "for x in a b c; do if test $x = b; then continue; fi; echo s-$x; done\nexit\n",
    );
    assert!(out.lines().any(|l| l == "s-a"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "s-b"), "s-b should be skipped: {out}");
    assert!(out.lines().any(|l| l == "s-c"), "stdout: {out}");
}

#[test]
fn for_variable_observable_after_loop() {
    let (out, _) = run("for x in one two three; do echo iter; done\necho final-$x\nexit\n");
    assert!(out.lines().any(|l| l == "final-three"), "stdout: {out}");
}

#[test]
fn for_invalid_variable_is_nonfatal_syntax_error() {
    let (out, err) = run("for 2x in a; do echo hi; done\necho still-alive\nexit\n");
    assert!(err.to_lowercase().contains("syntax error"), "stderr: {err}");
    assert!(out.lines().any(|l| l == "still-alive"), "stdout: {out}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test for_integration`
Expected: 13 tests pass.

If a test fails, inspect the actual stdout/stderr it prints. `for`-loop behavior was implemented in Tasks 1-2. Only adjust a test's assertion if it genuinely mismatches *correct* shell output, and explain why in your report. NEVER change shell behavior (in `src/`) to make a test pass.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: PASS — all tests, prior plus the 13 new.

- [ ] **Step 4: Commit**

```bash
git add tests/for_integration.rs
git commit -m "v20 task 3: end-to-end for-loop integration tests"
```

---

## Task 4: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v20 row to the status table**

Append after the v19 row, matching the table's column alignment:

```
| v20       | `for` loops (`for NAME in WORDS; do … done`)            |
```

- [ ] **Step 2: Add a feature note**

After the v19 multi-line-input block, add:

```markdown
**`for` loops (v20):**
`for NAME in WORD...; do LIST; done` runs the body once per word, with
`NAME` set to each word in turn. The word list is expanded once before
the loop — variables, command substitution, globs, and word-splitting
all apply, exactly as for command arguments (`for f in *.txt`, `for x
in $list`, `for n in $(seq 3)`). `break`/`continue` and multi-line form
work as for `while`. The no-`in` form (`for NAME; do … done`) runs the
body zero times — huck has no positional parameters for it to iterate.
After the loop `NAME` keeps its last value. C-style `for ((;;))` is not
implemented.
```

- [ ] **Step 3: Update the test-suite count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the `passed` counts across all lines. The README's Build-and-run section has a line `cargo test               # full test suite (NNN tests)`. Update `NNN` to the actual total. Expected ≈ 762 — use the real summed number.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "v20 task 4: README — for loops"
```

---

## Final review checkpoint

After Task 4:

- [ ] `cargo test` shows the expected total passing, 0 failing.
- [ ] `cargo clippy --all-targets -- -D warnings` introduces no new lints versus `main` (the codebase has ~38 pre-existing lints; only new ones count).
- [ ] Manual REPL smoke session: a `for` over a literal list, a multi-line `for`, a `for` over a glob and over `$(…)`, `break` and `continue` inside a `for`, a `for` nested in an `if`, a zero-iteration `for`, the loop variable observed after the loop, and an invalid variable name (`for 2x in a; do …`) producing a non-fatal syntax error.
- [ ] Confirm `if`/`while`/`until` and all single-line behaviour still work (the v17/v18/v19 suites are the proof).
- [ ] Final-review the whole branch as a single diff before merging to `main`.
```
