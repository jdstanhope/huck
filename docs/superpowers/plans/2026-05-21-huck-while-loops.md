# huck v18: `while` / `until` Loops Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `while`/`until` loops plus the `break`/`continue` builtins.

**Architecture:** `while`/`until` follow the v17 `if` pattern — a new `Command::While` variant, a `parse_while` recursive-descent function, a `run_while` executor function. `break`/`continue` add two `ExecOutcome` variants that the builtins produce, that propagate through the executor like `Exit`, and that `run_while` catches. No lexer change.

**Tech Stack:** Rust 2024 edition. No new dependencies.

**Reference:** Design spec at `docs/superpowers/specs/2026-05-21-huck-while-loops-design.md`.

---

## File Map

- **Modify:** `src/command.rs` — `Command::While`, `WhileClause`, four `Keyword` variants, `parse_while`, `expect_keyword` error parameter, `ParseError::UnterminatedLoop`
- **Modify:** `src/executor.rs` — `ExecOutcome::LoopBreak`/`LoopContinue`, `run_while`, `run_command` dispatch, loop-signal handling in `execute_sequence_body` and `run_if`
- **Modify:** `src/builtins.rs` — `break`/`continue` in `BUILTIN_NAMES` + dispatch
- **Modify:** `src/shell.rs` — `UnterminatedLoop` display; treat a top-level loop signal as `Continue(0)`
- **New:** `tests/while_integration.rs`
- **Modify:** `README.md` — v18 row, features note, builtins list, test count

---

## Task 1: AST and parser for `while` / `until`

Add `Command::While` / `WhileClause`, the four loop keywords, and `parse_while`. The executor `run_command` gets a `While` arm that `unreachable!`s until Task 3, so this task's tests are parser-only.

**Files:**
- Modify: `src/command.rs`
- Modify: `src/executor.rs` (one `run_command` arm)
- Modify: `src/shell.rs` (one `ParseError` display arm)

- [ ] **Step 1: Add the AST type**

In `src/command.rs`, add `Command::While` to the `Command` enum and a `WhileClause` struct (place it near `IfClause`):

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Command {
    Pipeline(Pipeline),
    If(Box<IfClause>),
    While(Box<WhileClause>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct WhileClause {
    pub condition: Sequence,
    pub body: Sequence,
    pub until: bool,
}
```

- [ ] **Step 2: Write failing parser tests**

Add to `src/command.rs` `#[cfg(test)] mod tests`. First a helper:

```rust
/// Extracts the WhileClause from a sequence whose first command is a While.
fn first_while(seq: &Sequence) -> &WhileClause {
    match &seq.first {
        Command::While(c) => c,
        other => panic!("expected a while, got {other:?}"),
    }
}
```

Then the tests:

```rust
#[test]
fn parse_simple_while() {
    // while a; do b; done
    let seq = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"), w_tok("b"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    let c = first_while(&seq);
    assert_eq!(c.until, false);
    assert_eq!(c.condition.first, Command::Pipeline(Pipeline { commands: vec![plain("a", &[])] }));
    assert_eq!(c.body.first, Command::Pipeline(Pipeline { commands: vec![plain("b", &[])] }));
}

#[test]
fn parse_until_sets_flag() {
    let seq = parse(vec![
        kw("until"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"), w_tok("b"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert_eq!(first_while(&seq).until, true);
}

#[test]
fn parse_while_andor_condition() {
    // while a && b; do c; done
    let seq = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::And), w_tok("b"),
        Token::Op(Operator::Semi),
        kw("do"), w_tok("c"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    let c = first_while(&seq);
    assert_eq!(c.condition.rest.len(), 1);
    assert_eq!(c.condition.rest[0].0, Connector::And);
}

#[test]
fn parse_while_multi_command_body() {
    let seq = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"), w_tok("b"), Token::Op(Operator::Semi), w_tok("c"),
        Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert_eq!(first_while(&seq).body.rest.len(), 1);
}

#[test]
fn parse_while_followed_by_command() {
    // while a; do b; done; echo
    let seq = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"), w_tok("b"), Token::Op(Operator::Semi),
        kw("done"), Token::Op(Operator::Semi), w_tok("echo"),
    ]).unwrap().unwrap();
    assert!(matches!(seq.first, Command::While(_)));
    assert_eq!(seq.rest.len(), 1);
    assert!(matches!(seq.rest[0].1, Command::Pipeline(_)));
}

#[test]
fn parse_nested_while() {
    // while a; do while b; do c; done; done
    let seq = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"),
        kw("while"), w_tok("b"), Token::Op(Operator::Semi),
        kw("do"), w_tok("c"), Token::Op(Operator::Semi),
        kw("done"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert!(matches!(first_while(&seq).body.first, Command::While(_)));
}

#[test]
fn parse_while_with_if_body() {
    // while a; do if b; then c; fi; done
    let seq = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"),
        kw("if"), w_tok("b"), Token::Op(Operator::Semi),
        kw("then"), w_tok("c"), Token::Op(Operator::Semi),
        kw("fi"), Token::Op(Operator::Semi),
        kw("done"),
    ]).unwrap().unwrap();
    assert!(matches!(first_while(&seq).body.first, Command::If(_)));
}

#[test]
fn parse_while_unterminated_is_error() {
    // while a; do b   (no done)
    let r = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"), w_tok("b"),
    ]);
    assert_eq!(r, Err(ParseError::UnterminatedLoop));
}

#[test]
fn parse_while_missing_do_is_error() {
    // while a; done
    let r = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::Semi), kw("done"),
    ]);
    assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))));
}

#[test]
fn parse_bare_do_is_unexpected_keyword() {
    assert!(matches!(
        parse(vec![kw("do"), w_tok("x")]),
        Err(ParseError::UnexpectedKeyword(_))
    ));
}

#[test]
fn parse_bare_done_is_unexpected_keyword() {
    assert!(matches!(
        parse(vec![kw("done")]),
        Err(ParseError::UnexpectedKeyword(_))
    ));
}

#[test]
fn parse_while_empty_condition_is_missing_command() {
    let r = parse(vec![
        kw("while"), Token::Op(Operator::Semi),
        kw("do"), w_tok("b"), Token::Op(Operator::Semi), kw("done"),
    ]);
    assert_eq!(r, Err(ParseError::MissingCommand));
}

#[test]
fn parse_while_background_in_body_is_error() {
    // `&` inside a loop body is rejected.
    let r = parse(vec![
        kw("while"), w_tok("a"), Token::Op(Operator::Semi),
        kw("do"), w_tok("b"), Token::Op(Operator::Background),
        kw("done"),
    ]);
    assert_eq!(r, Err(ParseError::UnexpectedBackground));
}

#[test]
fn parse_keyword_while_as_argument_is_literal() {
    let seq = parse(vec![w_tok("echo"), w_tok("while")]).unwrap().unwrap();
    assert_eq!(seq.first, Command::Pipeline(Pipeline {
        commands: vec![plain("echo", &["while"])],
    }));
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test parse_simple_while`
Expected: FAIL — `WhileClause` exists but `parse` does not produce it; `UnterminatedLoop` does not exist.

- [ ] **Step 4: Add the `UnterminatedLoop` ParseError variant**

Extend `ParseError` in `src/command.rs`:

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
}
```

- [ ] **Step 5: Add the four loop keywords**

Extend the `Keyword` enum and `keyword_of` / `Keyword::name` in `src/command.rs`:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Keyword {
    If, Then, Elif, Else, Fi,
    While, Until, Do, Done,
}

impl Keyword {
    fn name(self) -> &'static str {
        match self {
            Keyword::If => "if",
            Keyword::Then => "then",
            Keyword::Elif => "elif",
            Keyword::Else => "else",
            Keyword::Fi => "fi",
            Keyword::While => "while",
            Keyword::Until => "until",
            Keyword::Do => "do",
            Keyword::Done => "done",
        }
    }
}
```

In `keyword_of`, add to the `match text.as_str()`:

```rust
        "while" => Some(Keyword::While),
        "until" => Some(Keyword::Until),
        "do" => Some(Keyword::Do),
        "done" => Some(Keyword::Done),
```

- [ ] **Step 6: Give `expect_keyword` an error parameter**

Change `expect_keyword` so callers choose the error:

```rust
fn expect_keyword<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    expected: Keyword,
    on_missing: ParseError,
) -> Result<(), ParseError> {
    match iter.next() {
        Some(ref t) if keyword_of(t) == Some(expected) => Ok(()),
        _ => Err(on_missing),
    }
}
```

Update `parse_if`'s existing `expect_keyword` calls to pass `ParseError::UnterminatedIf` as the third argument (the calls for `If`, `Then`, `Fi`, and the `elif` loop's `Then`).

- [ ] **Step 7: Add `parse_while` and wire it into `parse_command`**

Add `parse_while` to `src/command.rs`:

```rust
/// Parses `while LIST; do LIST; done` or `until LIST; do LIST; done`.
/// The caller has already peeked the leading `while`/`until`.
fn parse_while<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<WhileClause, ParseError> {
    let until = match iter.next().as_ref().and_then(keyword_of) {
        Some(Keyword::While) => false,
        Some(Keyword::Until) => true,
        _ => unreachable!("parse_command guarantees a while/until keyword here"),
    };
    let condition = parse_sequence(iter, &[Keyword::Do])?;
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;
    let body = parse_sequence(iter, &[Keyword::Done])?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(WhileClause { condition, body, until })
}
```

In `parse_command`, add the `while`/`until` arm before the `Some(other)` catch-all:

```rust
fn parse_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
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

- [ ] **Step 8: Add the executor `run_command` arm**

In `src/executor.rs`, `run_command`'s `match` is non-exhaustive now. Add a placeholder arm:

```rust
        Command::While(_) => unreachable!("run_while lands in v18 task 3"),
```

(`WhileClause` does not need to be imported yet — Task 3 adds it.)

- [ ] **Step 9: Add the `shell.rs` parse-error message**

`src/shell.rs`'s parse-error display `match` is non-exhaustive now. Add an arm for `UnterminatedLoop`, matching the style of the `UnterminatedIf` arm:

```rust
ParseError::UnterminatedLoop => ": unterminated loop (expected 'do'/'done')".to_string(),
```

- [ ] **Step 10: Run the tests**

Run: `cargo test`
Expected: all parser tests pass — the 14 new `while` tests plus the 650 pre-existing. New total ≈ 664. No `while` is executed (the `run_command` arm `unreachable!`s); the new tests only call `parse` and inspect the AST.

- [ ] **Step 11: Commit**

```bash
git add src/command.rs src/executor.rs src/shell.rs
git commit -m "v18 task 1: AST and parser for while/until"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/while-loops`
- Baseline: 650 tests passing
- `kw`, `w_tok`, `plain` are existing `command.rs` test helpers. `Command`, `Pipeline`, `Connector`, `parse_sequence`, `keyword_of` all exist from v17.
- `parse_while` mirrors `parse_if`. `do`/`done` terminate the inner sequences via `parse_sequence`'s `stop_at`.

## Self-Review

- Do all 14 new `while` parser tests pass?
- Do all 650 pre-existing tests still pass (the `parse_if` `expect_keyword` calls updated to pass the new third argument)?
- Does `echo while` keep `while` as an argument?
- Is no `while` executed in any test?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 2: `break` / `continue` machinery

Add the `ExecOutcome::LoopBreak`/`LoopContinue` variants, the `break`/`continue` builtins, and the propagation/short-circuit logic. No `run_while` yet — a `break`/`continue` outside a loop propagates to the top level and is neutralized.

**Files:**
- Modify: `src/executor.rs`
- Modify: `src/builtins.rs`
- Modify: `src/shell.rs`

- [ ] **Step 1: Write failing tests**

Add to `src/builtins.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn builtin_break_returns_loop_break() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("break", &[], &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::LoopBreak));
}

#[test]
fn builtin_continue_returns_loop_continue() {
    let mut shell = Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("continue", &[], &mut out, &mut shell);
    assert!(matches!(outcome, ExecOutcome::LoopContinue));
}
```

Add to `src/executor.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn stray_break_at_top_level_is_harmless() {
    // `break` with no enclosing loop: the sequence stops, status 0,
    // the shell is unaffected.
    use crate::command::{ExecCommand, Pipeline};
    use crate::lexer::{Word, WordPart};
    let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
    let seq = Sequence {
        first: Command::Pipeline(Pipeline {
            commands: vec![SimpleCommand::Exec(ExecCommand {
                program: ww("break"),
                args: vec![],
                stdin: None,
                stdout: None,
                stderr: None,
            })],
        }),
        rest: vec![],
        background: false,
    };
    let mut shell = Shell::new();
    let (_out, status) = execute_capturing(&seq, &mut shell);
    assert_eq!(status, 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test builtin_break_returns_loop_break`
Expected: FAIL — `"break"` is not a builtin; `ExecOutcome::LoopBreak` does not exist.

- [ ] **Step 3: Add the `ExecOutcome` variants**

In `src/builtins.rs` (where `ExecOutcome` is defined), add two variants:

```rust
#[derive(Debug)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
    LoopBreak,
    LoopContinue,
}
```

- [ ] **Step 4: Add the `break`/`continue` builtins**

In `src/builtins.rs`:
- Add `"break"` and `"continue"` to the `BUILTIN_NAMES` constant.
- In `run_builtin`'s `match name`, add arms before the `_ =>` arm:

```rust
"break" => ExecOutcome::LoopBreak,
"continue" => ExecOutcome::LoopContinue,
```

(`break`/`continue` ignore their arguments in v18; no separate `builtin_*` function is needed — the outcome is the whole behavior.)

- [ ] **Step 5: Fix every non-exhaustive `ExecOutcome` match**

Run: `cargo build 2>&1 | head -50`

The two new variants make exhaustive `match`es on `ExecOutcome` non-exhaustive. Fix each:

- **`execute_sequence_body`** (`src/executor.rs`): it currently short-circuits with `if matches!(status, ExecOutcome::Exit(_)) { return status; }` after the first command and after each `rest` command. Extend BOTH checks to also short-circuit on the loop signals:

  ```rust
  if matches!(
      status,
      ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
  ) {
      return status;
  }
  ```

  So `echo a; break; echo b` runs `echo a`, then `break` stops the sequence.

- **`run_if`** (`src/executor.rs`): its condition checks use `if matches!(cond, ExecOutcome::Exit(_)) { return cond; }`. Extend each condition check (the main `if` condition and each `elif` condition) the same way — so a `break` in an `if` condition propagates:

  ```rust
  if matches!(
      cond,
      ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue
  ) {
      return cond;
  }
  ```

  `run_if`'s branch bodies are already `return execute_sequence_body(...)`, which carries a loop signal out unchanged — no change needed there.

- **Any exhaustive `match` that extracts a bare `i32` status** — e.g. `execute_capturing`'s `match outcome { ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c }`. Add an arm:

  ```rust
  ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => 0,
  ```

- **`src/shell.rs`** — wherever `process_line` / the REPL maps the final `ExecOutcome` to `$?` or a return code, a top-level `LoopBreak`/`LoopContinue` is treated as `Continue(0)`: add an arm setting last-status to 0 (do NOT exit the shell).

- **Any background-pipeline code in `executor.rs`** the compiler flags — map `LoopBreak`/`LoopContinue` to status 0.

Work through every error the compiler reports. The rule: `LoopBreak`/`LoopContinue` short-circuit a sequence like `Exit`, propagate through `run_if` like `Exit`, and map to status `0` wherever a plain exit code is needed.

- [ ] **Step 6: Run the full suite**

Run: `cargo test`
Expected: all tests pass — the 3 new tests plus everything else (≈ 667). `LoopBreak`/`LoopContinue` are produced by the builtins and handled everywhere; `run_while` (Task 3) is the only place that will *catch* them rather than propagate.

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs src/builtins.rs src/shell.rs
git commit -m "v18 task 2: break/continue builtins and ExecOutcome loop signals"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/while-loops`
- Baseline: ≈664 tests passing (after Task 1)
- `ExecOutcome` is defined in `src/builtins.rs`; `run_builtin` returns it.
- Until Task 3, a `break`/`continue` is never *caught* — it propagates to the top and is neutralized to status 0. The `stray_break_at_top_level_is_harmless` test confirms this.

## Self-Review

- Do the 3 new tests pass?
- Did you fix EVERY non-exhaustive `ExecOutcome` match the compiler flagged?
- Do all pre-existing tests still pass?

## Report Format

Status, the list of match sites you fixed, test count, commit SHA, any concerns.

---

## Task 3: Executor — `run_while`

Implement `run_while` and replace the `unreachable!` in `run_command`.

**Files:**
- Modify: `src/executor.rs`

- [ ] **Step 1: Write failing executor tests**

Add to `src/executor.rs` `#[cfg(test)] mod tests`. Reuse the `seq_of` / `echo_seq` / `cond_seq` helpers added in v17's executor tests if they exist; if not, add them (see v17's pattern). Then a helper to build a counting loop is overkill — use `cond_seq` and assignment-based bodies via small constructed sequences. Keep the unit tests focused on `run_while`'s control logic; the counting-loop behavior is covered end-to-end in Task 4.

```rust
use crate::command::WhileClause;

/// A Sequence wrapping a single `while`/`until` clause.
fn while_seq(clause: WhileClause) -> Sequence {
    Sequence { first: Command::While(Box::new(clause)), rest: vec![], background: false }
}

/// A one-pipeline Sequence running the `break` builtin.
fn break_seq() -> Sequence {
    use crate::command::{ExecCommand, Pipeline};
    use crate::lexer::{Word, WordPart};
    let ww = |s: &str| Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]);
    Sequence {
        first: Command::Pipeline(Pipeline {
            commands: vec![SimpleCommand::Exec(ExecCommand {
                program: ww("break"),
                args: vec![],
                stdin: None, stdout: None, stderr: None,
            })],
        }),
        rest: vec![],
        background: false,
    }
}

#[test]
fn while_false_condition_runs_body_zero_times() {
    // while (test 1 -eq 0); do echo x; done  — condition false, body never runs.
    let clause = WhileClause {
        condition: cond_seq(false),
        body: echo_seq("x"),
        until: false,
    };
    let mut shell = Shell::new();
    let (out, status) = execute_capturing(&while_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
    assert_eq!(status, 0);
}

#[test]
fn while_true_body_breaks_runs_once() {
    // while (true); do break; done  — body runs once, `break` ends the loop.
    let clause = WhileClause {
        condition: cond_seq(true),
        body: break_seq(),
        until: false,
    };
    let mut shell = Shell::new();
    let (_out, status) = execute_capturing(&while_seq(clause), &mut shell);
    assert_eq!(status, 0);
    // If `break` were not caught, this would loop forever — reaching this
    // assertion at all proves the loop terminated.
}

#[test]
fn until_true_condition_runs_body_zero_times() {
    // until (test 0 -eq 0); do echo x; done — condition true, so `until`
    // stops immediately.
    let clause = WhileClause {
        condition: cond_seq(true),
        body: echo_seq("x"),
        until: true,
    };
    let mut shell = Shell::new();
    let (out, _) = execute_capturing(&while_seq(clause), &mut shell);
    assert_eq!(out.trim(), "");
}
```

(`cond_seq`/`echo_seq` are the v17 executor-test helpers: `cond_seq(true)` is a condition exiting 0, `cond_seq(false)` exits 1, `echo_seq("x")` prints `x`. If they are not present, add them as in v17's plan.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test while_true_body_breaks_runs_once`
Expected: FAIL — `run_command`'s `Command::While` arm is still `unreachable!` (panics).

- [ ] **Step 3: Implement `run_while` and wire it in**

In `src/executor.rs`, add `WhileClause` to the `use crate::command::{...}` import. Replace the `Command::While(_) => unreachable!(...)` arm:

```rust
fn run_command(cmd: &Command, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    match cmd {
        Command::Pipeline(p) => run_pipeline(p, shell, sink),
        Command::If(clause) => run_if(clause, shell, sink),
        Command::While(clause) => run_while(clause, shell, sink),
    }
}

/// Runs a `while`/`until` loop. The body runs while the condition's
/// exit status satisfies the loop's polarity. `break` ends the loop;
/// `continue` jumps to the next condition test; `exit` propagates; a
/// pending SIGINT (`Ctrl-C`) ends the loop with status 130.
fn run_while(clause: &WhileClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    use std::sync::atomic::Ordering;
    let mut last = ExecOutcome::Continue(0);
    loop {
        if shell.sigint_flag.load(Ordering::Relaxed) {
            shell.sigint_flag.store(false, Ordering::Relaxed);
            return ExecOutcome::Continue(130);
        }
        let cond = execute_sequence_body(&clause.condition, shell, sink);
        let keep_going = match cond {
            ExecOutcome::Exit(_) | ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => {
                return cond;
            }
            ExecOutcome::Continue(c) => {
                if clause.until { c != 0 } else { c == 0 }
            }
        };
        if !keep_going {
            break;
        }
        match execute_sequence_body(&clause.body, shell, sink) {
            ExecOutcome::Exit(code) => return ExecOutcome::Exit(code),
            ExecOutcome::LoopBreak => {
                last = ExecOutcome::Continue(0);
                break;
            }
            ExecOutcome::LoopContinue => {
                last = ExecOutcome::Continue(0);
                // fall through — the loop re-tests the condition
            }
            ExecOutcome::Continue(c) => {
                last = ExecOutcome::Continue(c);
            }
        }
    }
    last
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: all tests pass — the 3 new executor tests plus everything else (≈ 670).

- [ ] **Step 5: Manual smoke test**

```bash
cargo build --release
~/projects/shuck/target/release/huck <<'EOF'
i=0; while test $i -lt 3; do echo iter-$i; i=$((i+1)); done
n=5; until test $n -eq 0; do echo down-$n; n=$((n-1)); done
i=0; while true; do echo once; break; done
i=0; while test $i -lt 5; do i=$((i+1)); if test $i -eq 3; then continue; fi; echo got-$i; done
exit
EOF
```

Expected output: `iter-0 iter-1 iter-2`, then `down-5 down-4 down-3 down-2 down-1`, then `once`, then `got-1 got-2 got-4 got-5` (note `got-3` is skipped by `continue`).

Then verify Ctrl-C interrupts an infinite loop — run `huck` interactively, type `while true; do echo x; done`, press Ctrl-C, confirm the prompt returns. Report what you observe.

- [ ] **Step 6: Commit**

```bash
git add src/executor.rs
git commit -m "v18 task 3: executor run_while — while/until loop execution"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/while-loops`
- Baseline: ≈667 tests passing (after Task 2)
- `shell.sigint_flag` is an `Arc<AtomicBool>` on `Shell`, set by the SIGINT handler — the same flag `wait` (v6) polls. `run_while` checks it each iteration.
- `execute_sequence_body` already short-circuits on `LoopBreak`/`LoopContinue` (Task 2), so a loop signal from the condition or body surfaces as `run_while`'s `execute_sequence_body` return value.
- The mutual recursion `run_command` → `run_while` → `execute_sequence_body` → `run_command` is fine.

## Self-Review

- Do the 3 new executor tests pass?
- Did the smoke test produce the expected output, including `continue` skipping `got-3`?
- Does Ctrl-C interrupt `while true; do echo x; done`?
- Do all pre-existing tests still pass?

## Report Format

Status, smoke test output, test count, commit SHA, any concerns.

---

## Task 4: End-to-end integration tests

**Files:**
- Create: `tests/while_integration.rs`

- [ ] **Step 1: Create the test file**

Create `tests/while_integration.rs`:

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
fn while_counting_loop() {
    let (out, _) = run("i=0; while test $i -lt 3; do echo $i; i=$((i+1)); done\nexit\n");
    let nums: Vec<&str> = out.lines().filter(|l| *l == "0" || *l == "1" || *l == "2").collect();
    assert_eq!(nums, vec!["0", "1", "2"], "stdout: {out}");
}

#[test]
fn until_loop() {
    let (out, _) = run("n=3; until test $n -eq 0; do echo n$n; n=$((n-1)); done\nexit\n");
    assert!(out.lines().any(|l| l == "n3"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "n1"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "n0"), "n0 should not appear: {out}");
}

#[test]
fn break_exits_loop_early() {
    let (out, _) = run(
        "i=0; while test $i -lt 100; do echo at-$i; i=$((i+1)); if test $i -eq 2; then break; fi; done\nexit\n",
    );
    assert!(out.lines().any(|l| l == "at-0"), "stdout: {out}");
    assert!(out.lines().any(|l| l == "at-1"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "at-2"), "loop should have broken: {out}");
}

#[test]
fn continue_skips_iteration() {
    let (out, _) = run(
        "i=0; while test $i -lt 4; do i=$((i+1)); if test $i -eq 2; then continue; fi; echo v$i; done\nexit\n",
    );
    assert!(out.lines().any(|l| l == "v1"), "stdout: {out}");
    assert!(!out.lines().any(|l| l == "v2"), "v2 should be skipped: {out}");
    assert!(out.lines().any(|l| l == "v3"), "stdout: {out}");
}

#[test]
fn while_true_with_break() {
    let (out, _) = run("while true; do echo once; break; done\nexit\n");
    let count = out.lines().filter(|l| *l == "once").count();
    assert_eq!(count, 1, "stdout: {out}");
}

#[test]
fn nested_while() {
    let (out, _) = run(
        "i=0; while test $i -lt 2; do j=0; while test $j -lt 2; do echo $i-$j; j=$((j+1)); done; i=$((i+1)); done\nexit\n",
    );
    for pair in ["0-0", "0-1", "1-0", "1-1"] {
        assert!(out.lines().any(|l| l == pair), "missing {pair}: {out}");
    }
}

#[test]
fn stray_break_is_harmless() {
    let (out, _) = run("break\necho alive\nexit\n");
    assert!(out.lines().any(|l| l == "alive"), "stdout: {out}");
}

#[test]
fn while_loop_status_after() {
    // The loop's last body command is `test 1 -eq 2` (false); the loop's
    // exit status, checked right after, is 1.
    let (out, _) = run("i=0; while test $i -lt 1; do i=$((i+1)); test 1 -eq 2; done\necho $?\nexit\n");
    assert!(out.lines().any(|l| l == "1"), "stdout: {out}");
}

#[test]
fn unterminated_while_is_syntax_error() {
    let (_, err) = run("while test 1 -eq 1; do echo x\nexit\n");
    assert!(err.to_lowercase().contains("syntax error"), "stderr: {err}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test while_integration`
Expected: 9 tests pass.

If a test fails, examine the actual output. Shell behavior was verified in Tasks 1-3; if an assertion needs adjusting to match real (correct) output, do so and report it — do NOT change shell behavior.

- [ ] **Step 3: Run the full suite**

Run: `cargo test`
Expected: all tests pass (≈ 679).

- [ ] **Step 4: Commit**

```bash
git add tests/while_integration.rs
git commit -m "v18 task 4: end-to-end while/until integration tests"
```

## Context

- Working directory: `/home/john/projects/shuck`
- Branch: `feature/while-loops`
- Baseline: ≈670 tests passing
- `test` (v16), `if` (v17), and `$((...))` (v11) are the tools these loops use. The binary is `huck`.

## Self-Review

- Do all 9 integration tests pass?
- Does `continue_skips_iteration` confirm `v2` is skipped?
- Does the full suite pass?

## Report Format

Status, test count, commit SHA, any concerns.

---

## Task 5: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add the v18 row to the status table**

Append after the v17 row:

```
| v18       | `while`/`until` loops (`break`, `continue`)             |
```

Match the table's column alignment.

- [ ] **Step 2: Add a features note**

After the v17 `if`-control-flow block, add:

```markdown
**`while` / `until` loops (v18):**
`while LIST; do LIST; done` runs the body while the condition's exit
status is 0; `until` runs it while the condition is non-zero. `break`
exits the innermost loop and `continue` skips to its next iteration.
An infinite `while true; do …; done` is interruptible with Ctrl-C.
Loops are sequence-level compound commands — they compose with `;`,
`&&`, `||` and nest. Single-line form only (multi-line `if`/loops are
a later iteration); `break N` / `continue N` are not implemented.
```

- [ ] **Step 3: Add `break` and `continue` to the Builtins list**

Find the builtins enumeration in `README.md` and add `break` and `continue`.

- [ ] **Step 4: Update the Not-yet-implemented section**

The control-flow item currently reads `control flow (while/until/for/case)`. Update it to `control flow (for/case)` and functions — drop `while`/`until`.

- [ ] **Step 5: Update the test count**

Run: `cargo test 2>&1 | grep 'test result'` and sum the `passed` counts. Update the `cargo test               # full test suite (NNN tests)` line. Expected ≈ 679 — use the actual number.

- [ ] **Step 6: Commit**

```bash
git add README.md
git commit -m "v18 task 5: README — add v18 row and while-loops section"
```

---

## Final review checkpoint

After Task 5:

- [ ] `cargo test` shows the expected total passing, 0 failing
- [ ] `cargo clippy -- -D warnings` is clean (or any new warnings are intentional)
- [ ] Manual REPL smoke session: a counting `while`, an `until`, `break`, `continue`, a nested loop, `break` from inside an `if` inside a `while`, an infinite `while true` interrupted with Ctrl-C, a stray top-level `break`, and a syntax error (`while x; do y`)
- [ ] Final review the whole branch as a single diff before merging to main
