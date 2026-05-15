# shuck Command Sequencing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `&&`, `||`, and `;` sequencing operators to `shuck`, executing pipelines left-to-right with short-circuit evaluation.

**Architecture:** The data flow becomes `&str -> Vec<Token> -> Sequence -> ExecOutcome`. A `Sequence` is one pipeline plus a list of `(Connector, Pipeline)` pairs. The lexer adds `Op(And)` / `Op(Or)` / `Op(Semi)` with lookahead on `|` and `&`; the parser is split into an outer `parse` that builds `Sequence` and a private `parse_pipeline` helper (the previous parser body); the executor gains a top-level `execute(&Sequence)` that drives a left-to-right loop with short-circuit on `Exit`.

**Tech Stack:** Rust (edition 2024). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-15-shuck-sequencing-design.md`

---

## File Structure

| File | Change |
|------|--------|
| `src/lexer.rs` | Add `Operator::And`/`Or`/`Semi`; `|` and `&` lookahead; `LexError::BareAmpersand`; new tests. |
| `src/command.rs` | Add `Connector`, `Sequence`; split parser into outer `parse` + private `parse_pipeline`; `parse` returns `Result<Option<Sequence>, ParseError>`. |
| `src/executor.rs` | Rename existing private `run_pipeline` (multi-stage helper) → `run_multi_stage`; rename existing `pub execute(&Pipeline)` body → private `run_pipeline(&Pipeline)`; add new `pub execute(&Sequence)`. |
| `src/shell.rs` | `process_line` handles `Sequence` and the new `BareAmpersand` lex error. |
| `src/builtins.rs` | Unchanged. |
| `src/main.rs` | Unchanged. |

**Why the task order:** This is another interface migration. Each task leaves the crate compiling and all unit tests green. Task 1 lands the lexer change and a deliberately-**interim** parser arm that returns `MissingCommand` on sequencing operators. Task 2 replaces the interim parser with the real `Sequence`-building parser and updates the executor and shell to speak `Sequence` (with the executor temporarily refusing to actually sequence — it prints "not yet implemented" if `rest` is non-empty). Task 3 wires up the real left-to-right loop in `execute(&Sequence)`. Task 4 is a full automated smoke test. Per-task verification is `cargo test` (binary-only crate — never `cargo test --lib`).

---

## Task 1: Lexer recognizes sequencing operators

The lexer learns three new operators. The parser and shell get the minimum changes needed to keep the crate compiling and to surface the new lex error.

**Files:**
- Modify: `src/lexer.rs` (full replacement)
- Modify: `src/command.rs` (two small edits)
- Modify: `src/shell.rs` (one edit + a new helper)

- [ ] **Step 1: Replace `src/lexer.rs` entirely with this**

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
}

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
}

#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Op(Operator),
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if has_token {
                tokens.push(Token::Word(std::mem::take(&mut current)));
                has_token = false;
            }
            continue;
        }

        match c {
            '\'' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '"' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('"' | '\\')) => current.push(esc),
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some(ch) => current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '\\' => {
                has_token = true;
                match chars.next() {
                    Some(ch) => current.push(ch),
                    None => current.push('\\'),
                }
            }
            '|' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::Or));
                } else {
                    tokens.push(Token::Op(Operator::Pipe));
                }
            }
            '&' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::And));
                } else {
                    return Err(LexError::BareAmpersand);
                }
            }
            ';' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::Semi));
            }
            '<' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::RedirIn));
            }
            '>' => {
                if has_token {
                    tokens.push(Token::Word(std::mem::take(&mut current)));
                    has_token = false;
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
            }
            // `2>` / `2>>` — only when the `2` would otherwise start a new
            // word (no current word being built). A `2` inside or appended to
            // a word, e.g. `x2>f`, is ordinary text.
            '2' if !has_token && chars.peek() == Some(&'>') => {
                chars.next(); // consume the '>'
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirErrAppend));
                } else {
                    tokens.push(Token::Op(Operator::RedirErr));
                }
            }
            other => {
                has_token = true;
                current.push(other);
            }
        }
    }

    if has_token {
        tokens.push(Token::Word(current));
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an expected token list made entirely of words.
    fn words(parts: &[&str]) -> Vec<Token> {
        parts.iter().map(|w| Token::Word(w.to_string())).collect()
    }

    // ----- existing v2 lexer tests (must keep passing) -----

    #[test]
    fn tokenize_simple_command() {
        assert_eq!(tokenize("ls -la").unwrap(), words(&["ls", "-la"]));
    }

    #[test]
    fn tokenize_empty_input() {
        assert_eq!(tokenize("").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_only_whitespace() {
        assert_eq!(tokenize("   \t  ").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_single_quotes() {
        assert_eq!(
            tokenize("echo 'hello world'").unwrap(),
            words(&["echo", "hello world"])
        );
    }

    #[test]
    fn tokenize_double_quotes() {
        assert_eq!(
            tokenize("echo \"hello world\"").unwrap(),
            words(&["echo", "hello world"])
        );
    }

    #[test]
    fn tokenize_double_quote_escape() {
        assert_eq!(tokenize(r#"echo "a\"b""#).unwrap(), words(&["echo", "a\"b"]));
    }

    #[test]
    fn tokenize_backslash_escape_outside_quotes() {
        assert_eq!(tokenize(r"echo a\ b").unwrap(), words(&["echo", "a b"]));
    }

    #[test]
    fn tokenize_trailing_backslash_is_literal() {
        assert_eq!(tokenize(r"echo a\").unwrap(), words(&["echo", r"a\"]));
    }

    #[test]
    fn tokenize_adjacent_runs_concatenate() {
        assert_eq!(tokenize(r#"foo"bar baz""#).unwrap(), words(&["foobar baz"]));
    }

    #[test]
    fn tokenize_single_quotes_preserve_backslash() {
        assert_eq!(tokenize(r"echo 'a\b'").unwrap(), words(&["echo", r"a\b"]));
    }

    #[test]
    fn tokenize_empty_quotes_produce_empty_token() {
        assert_eq!(tokenize("''").unwrap(), words(&[""]));
    }

    #[test]
    fn tokenize_unterminated_single_quote() {
        assert_eq!(
            tokenize("echo 'oops").unwrap_err(),
            LexError::UnterminatedQuote
        );
    }

    #[test]
    fn tokenize_unterminated_double_quote() {
        assert_eq!(
            tokenize("echo \"oops").unwrap_err(),
            LexError::UnterminatedQuote
        );
    }

    #[test]
    fn tokenize_pipe_with_spaces() {
        assert_eq!(
            tokenize("a | b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Pipe),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_pipe_without_spaces() {
        assert_eq!(
            tokenize("a|b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Pipe),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_out() {
        assert_eq!(
            tokenize("ls > f").unwrap(),
            vec![
                Token::Word("ls".to_string()),
                Token::Op(Operator::RedirOut),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_out_without_spaces() {
        assert_eq!(
            tokenize("ls>f").unwrap(),
            vec![
                Token::Word("ls".to_string()),
                Token::Op(Operator::RedirOut),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_append() {
        assert_eq!(
            tokenize("ls >> f").unwrap(),
            vec![
                Token::Word("ls".to_string()),
                Token::Op(Operator::RedirAppend),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_in() {
        assert_eq!(
            tokenize("cat < f").unwrap(),
            vec![
                Token::Word("cat".to_string()),
                Token::Op(Operator::RedirIn),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_stderr() {
        assert_eq!(
            tokenize("cmd 2> f").unwrap(),
            vec![
                Token::Word("cmd".to_string()),
                Token::Op(Operator::RedirErr),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_stderr_append() {
        assert_eq!(
            tokenize("cmd 2>> f").unwrap(),
            vec![
                Token::Word("cmd".to_string()),
                Token::Op(Operator::RedirErrAppend),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_two_in_word_is_not_stderr_operator() {
        assert_eq!(
            tokenize("x2>f").unwrap(),
            vec![
                Token::Word("x2".to_string()),
                Token::Op(Operator::RedirOut),
                Token::Word("f".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_two_not_followed_by_redirect_is_a_word() {
        assert_eq!(tokenize("2 foo").unwrap(), words(&["2", "foo"]));
    }

    #[test]
    fn tokenize_quoted_operators_stay_words() {
        assert_eq!(
            tokenize(r#"echo "|" ">""#).unwrap(),
            words(&["echo", "|", ">"])
        );
    }

    #[test]
    fn tokenize_escaped_operators_stay_words() {
        assert_eq!(tokenize(r"echo \| \>").unwrap(), words(&["echo", "|", ">"]));
    }

    #[test]
    fn tokenize_pipeline_with_redirects() {
        assert_eq!(
            tokenize("a < in | b > out").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::RedirIn),
                Token::Word("in".to_string()),
                Token::Op(Operator::Pipe),
                Token::Word("b".to_string()),
                Token::Op(Operator::RedirOut),
                Token::Word("out".to_string()),
            ]
        );
    }

    // ----- new: sequencing operators -----

    #[test]
    fn tokenize_or_with_spaces() {
        assert_eq!(
            tokenize("a || b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Or),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_or_without_spaces() {
        assert_eq!(
            tokenize("a||b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Or),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_and_with_spaces() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::And),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_and_without_spaces() {
        assert_eq!(
            tokenize("a&&b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::And),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_is_error() {
        assert_eq!(tokenize("a & b").unwrap_err(), LexError::BareAmpersand);
    }

    #[test]
    fn tokenize_bare_ampersand_at_end_is_error() {
        assert_eq!(tokenize("a &").unwrap_err(), LexError::BareAmpersand);
    }

    #[test]
    fn tokenize_semicolon_with_spaces() {
        assert_eq!(
            tokenize("a ; b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Semi),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_semicolon_without_spaces() {
        assert_eq!(
            tokenize("a;b").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::Semi),
                Token::Word("b".to_string()),
            ]
        );
    }

    #[test]
    fn tokenize_quoted_sequencing_operators_stay_words() {
        assert_eq!(
            tokenize(r#"echo "&&" "||" ";""#).unwrap(),
            words(&["echo", "&&", "||", ";"])
        );
    }

    #[test]
    fn tokenize_escaped_sequencing_operators_stay_words() {
        // `\&` is just `&` (literal); two `\&` make `&&` (the literal word, not the op).
        assert_eq!(
            tokenize(r"echo \&\& \|\| \;").unwrap(),
            words(&["echo", "&&", "||", ";"])
        );
    }

    #[test]
    fn tokenize_combined_sequencing_operators() {
        assert_eq!(
            tokenize("a && b || c ; d").unwrap(),
            vec![
                Token::Word("a".to_string()),
                Token::Op(Operator::And),
                Token::Word("b".to_string()),
                Token::Op(Operator::Or),
                Token::Word("c".to_string()),
                Token::Op(Operator::Semi),
                Token::Word("d".to_string()),
            ]
        );
    }
}
```

- [ ] **Step 2: Update `src/command.rs` (two small edits)**

The current parser's outer match has a catch-all `Token::Op(op) => { /* redirect */ }` arm whose inner match on `op` is exhaustive over the existing `Operator` variants. The lexer now produces three new variants. Both changes are needed for the parser to compile.

**Edit 1 — add an outer arm catching the new ops** (between the existing `Token::Op(Operator::Pipe)` arm and the catch-all `Token::Op(op)` arm). Replace this block:

```rust
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(Command {
                    program: prog,
                    args: std::mem::take(&mut args),
                    stdin: stdin.take(),
                    stdout: stdout.take(),
                    stderr: stderr.take(),
                });
            }
            Token::Op(op) => {
```

with:

```rust
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(Command {
                    program: prog,
                    args: std::mem::take(&mut args),
                    stdin: stdin.take(),
                    stdout: stdout.take(),
                    stderr: stderr.take(),
                });
            }
            Token::Op(Operator::And | Operator::Or | Operator::Semi) => {
                // INTERIM (made real in Task 2): sequencing operators are not
                // yet parsed; report as a syntax error.
                return Err(ParseError::MissingCommand);
            }
            Token::Op(op) => {
```

**Edit 2 — extend the inner unreachable arm** so the inner match stays exhaustive. Replace:

```rust
                    Operator::Pipe => unreachable!("Pipe is handled in the arm above"),
```

with:

```rust
                    Operator::Pipe | Operator::And | Operator::Or | Operator::Semi => {
                        unreachable!("handled in the outer arms");
                    }
```

**Edit 3 — add one interim test** at the end of the existing `#[cfg(test)] mod tests` block, just before the closing `}` of `mod tests`:

```rust
    #[test]
    fn parse_sequencing_op_is_interim_missing_command() {
        // INTERIM (deleted in Task 2): the parser does not yet handle sequencing.
        assert_eq!(
            parse(vec![w("a"), Token::Op(Operator::Semi), w("b")]),
            Err(ParseError::MissingCommand)
        );
    }
```

- [ ] **Step 3: Update `src/shell.rs`**

The current `LexError` match has a single arm for `UnterminatedQuote`. Adding `BareAmpersand` makes the match non-exhaustive — handle both via a small helper.

**Edit 1 — broaden the lex-error arm in `process_line`.** Replace this block:

```rust
        Err(LexError::UnterminatedQuote) => {
            eprintln!("shuck: syntax error: unterminated quote");
            return ExecOutcome::Continue(2);
        }
```

with:

```rust
        Err(e) => {
            eprintln!("shuck: syntax error: {}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
```

**Edit 2 — add the helper.** Below the existing `parse_error_message` function (at the bottom of the file), append:

```rust
fn lex_error_message(error: LexError) -> &'static str {
    match error {
        LexError::UnterminatedQuote => "unterminated quote",
        LexError::BareAmpersand => "unexpected '&'",
    }
}
```

- [ ] **Step 4: Verify the crate builds and all tests pass**

Run: `cargo build`
Expected: PASS — no warnings.

Run: `cargo test`
Expected: PASS — 64 tests pass (38 lexer + 19 command + 7 builtins).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs src/command.rs src/shell.rs
git commit -m "feat: lexer recognizes && || ; operators"
```

---

## Task 2: Sequence parser, Sequence-aware executor, shell wiring

Replace the interim parser with the real one, building a `Sequence`. Rename the executor functions and add a top-level `execute(&Sequence)` whose body is an **interim** stub that runs only the first pipeline and reports "not yet implemented" for anything more. Task 3 makes the executor real.

**Files:**
- Modify: `src/command.rs` (full replacement)
- Modify: `src/executor.rs` (full replacement)
- Modify: `src/shell.rs` (one edit: variable rename)

- [ ] **Step 1: Replace `src/command.rs` entirely with this**

```rust
use crate::lexer::{Operator, Token};

#[derive(Debug, PartialEq, Eq)]
pub enum Redirect {
    Truncate(String), // > file   (and the target form of 2>)
    Append(String),   // >> file  (and the target form of 2>>)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>, // invariant: never empty
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Connector {
    Semi, // ;
    And,  // &&
    Or,   // ||
}

#[derive(Debug, PartialEq, Eq)]
pub struct Sequence {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }

    let mut iter = tokens.into_iter().peekable();
    let first = parse_pipeline(&mut iter)?;
    let mut rest = Vec::new();

    while let Some(token) = iter.next() {
        let connector = match token {
            Token::Op(Operator::Semi) => Connector::Semi,
            Token::Op(Operator::And) => Connector::And,
            Token::Op(Operator::Or) => Connector::Or,
            _ => unreachable!("parse_pipeline returns only at a sequencing op or end"),
        };
        // Trailing `;` is allowed: stop here if there's nothing after it.
        if matches!(connector, Connector::Semi) && iter.peek().is_none() {
            break;
        }
        let pipeline = parse_pipeline(&mut iter)?;
        rest.push((connector, pipeline));
    }

    Ok(Some(Sequence { first, rest }))
}

/// Parses one pipeline from the iterator. Stops at — without consuming — the
/// next sequencing operator (`;`, `&&`, `||`) or end of input. Returns
/// `Err(ParseError::MissingCommand)` if the pipeline ended with no program.
fn parse_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> {
    let mut commands: Vec<Command> = Vec::new();

    // Builder state for the command currently being assembled.
    let mut program: Option<String> = None;
    let mut args: Vec<String> = Vec::new();
    let mut stdin: Option<String> = None;
    let mut stdout: Option<Redirect> = None;
    let mut stderr: Option<Redirect> = None;

    while let Some(token) = iter.peek() {
        if matches!(
            token,
            Token::Op(Operator::Semi | Operator::And | Operator::Or)
        ) {
            // Don't consume — the outer loop handles it.
            break;
        }
        let token = iter.next().unwrap();
        match token {
            Token::Word(word) => {
                if program.is_none() {
                    program = Some(word);
                } else {
                    args.push(word);
                }
            }
            Token::Op(Operator::Pipe) => {
                let prog = program.take().ok_or(ParseError::MissingCommand)?;
                commands.push(Command {
                    program: prog,
                    args: std::mem::take(&mut args),
                    stdin: stdin.take(),
                    stdout: stdout.take(),
                    stderr: stderr.take(),
                });
            }
            Token::Op(op) => {
                // A redirect operator: the next token must be a filename word.
                let target = match iter.next() {
                    Some(Token::Word(word)) => word,
                    Some(Token::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                    None => return Err(ParseError::MissingRedirectTarget),
                };
                match op {
                    Operator::RedirIn => stdin = Some(target),
                    Operator::RedirOut => stdout = Some(Redirect::Truncate(target)),
                    Operator::RedirAppend => stdout = Some(Redirect::Append(target)),
                    Operator::RedirErr => stderr = Some(Redirect::Truncate(target)),
                    Operator::RedirErrAppend => stderr = Some(Redirect::Append(target)),
                    Operator::Pipe | Operator::And | Operator::Or | Operator::Semi => {
                        unreachable!("handled in the outer arms");
                    }
                }
            }
        }
    }

    let prog = program.ok_or(ParseError::MissingCommand)?;
    commands.push(Command {
        program: prog,
        args,
        stdin,
        stdout,
        stderr,
    });

    Ok(Pipeline { commands })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Token {
        Token::Word(s.to_string())
    }

    /// Builds a command with no redirections.
    fn plain(program: &str, args: &[&str]) -> Command {
        Command {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }

    /// Builds a sequence with a single pipeline (no sequencing operators).
    fn one_pipeline(commands: Vec<Command>) -> Sequence {
        Sequence {
            first: Pipeline { commands },
            rest: vec![],
        }
    }

    // ----- single-pipeline cases (regressions from v2) -----

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), Ok(None));
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w("ls")]),
            Ok(Some(one_pipeline(vec![plain("ls", &[])])))
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w("ls"), w("-la"), w("/tmp")]),
            Ok(Some(one_pipeline(vec![plain("ls", &["-la", "/tmp"])])))
        );
    }

    #[test]
    fn parse_redirect_out() {
        let seq = parse(vec![w("ls"), Token::Op(Operator::RedirOut), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(
            seq.first.commands[0].stdout,
            Some(Redirect::Truncate("f".to_string()))
        );
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_redirect_append() {
        let seq = parse(vec![w("ls"), Token::Op(Operator::RedirAppend), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(
            seq.first.commands[0].stdout,
            Some(Redirect::Append("f".to_string()))
        );
    }

    #[test]
    fn parse_redirect_in() {
        let seq = parse(vec![w("cat"), Token::Op(Operator::RedirIn), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands[0].stdin, Some("f".to_string()));
    }

    #[test]
    fn parse_redirect_stderr() {
        let seq = parse(vec![w("cmd"), Token::Op(Operator::RedirErr), w("e")])
            .unwrap()
            .unwrap();
        assert_eq!(
            seq.first.commands[0].stderr,
            Some(Redirect::Truncate("e".to_string()))
        );
    }

    #[test]
    fn parse_redirect_stderr_append() {
        let seq = parse(vec![w("cmd"), Token::Op(Operator::RedirErrAppend), w("e")])
            .unwrap()
            .unwrap();
        assert_eq!(
            seq.first.commands[0].stderr,
            Some(Redirect::Append("e".to_string()))
        );
    }

    #[test]
    fn parse_two_stage_pipeline() {
        let seq = parse(vec![w("a"), Token::Op(Operator::Pipe), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[]), plain("b", &[])]);
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_three_stage_pipeline() {
        let seq = parse(vec![
            w("a"),
            Token::Op(Operator::Pipe),
            w("b"),
            Token::Op(Operator::Pipe),
            w("c"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(seq.first.commands.len(), 3);
    }

    #[test]
    fn parse_pipeline_with_redirects_on_stages() {
        // a < in | b > out
        let seq = parse(vec![
            w("a"),
            Token::Op(Operator::RedirIn),
            w("in"),
            Token::Op(Operator::Pipe),
            w("b"),
            Token::Op(Operator::RedirOut),
            w("out"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(seq.first.commands[0].stdin, Some("in".to_string()));
        assert_eq!(
            seq.first.commands[1].stdout,
            Some(Redirect::Truncate("out".to_string()))
        );
    }

    #[test]
    fn parse_last_redirect_of_a_kind_wins() {
        let seq = parse(vec![
            w("ls"),
            Token::Op(Operator::RedirOut),
            w("a"),
            Token::Op(Operator::RedirOut),
            w("b"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            seq.first.commands[0].stdout,
            Some(Redirect::Truncate("b".to_string()))
        );
    }

    #[test]
    fn parse_leading_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Pipe), w("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_trailing_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![w("a"), Token::Op(Operator::Pipe)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_pipe_is_missing_command() {
        // Two consecutive Op(Pipe) — not Op(Or) — at the parser level.
        assert_eq!(
            parse(vec![
                w("a"),
                Token::Op(Operator::Pipe),
                Token::Op(Operator::Pipe),
                w("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_program_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::RedirOut), w("f")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_target_is_error() {
        assert_eq!(
            parse(vec![w("ls"), Token::Op(Operator::RedirOut)]),
            Err(ParseError::MissingRedirectTarget)
        );
    }

    #[test]
    fn parse_redirect_target_is_operator_is_error() {
        assert_eq!(
            parse(vec![
                w("ls"),
                Token::Op(Operator::RedirOut),
                Token::Op(Operator::Pipe),
                w("b"),
            ]),
            Err(ParseError::RedirectTargetIsOperator)
        );
    }

    // ----- new: command sequencing -----

    #[test]
    fn parse_semicolon_sequence() {
        let seq = parse(vec![w("a"), Token::Op(Operator::Semi), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[])]);
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
        assert_eq!(seq.rest[0].1.commands, vec![plain("b", &[])]);
    }

    #[test]
    fn parse_and_sequence() {
        let seq = parse(vec![w("a"), Token::Op(Operator::And), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_or_sequence() {
        let seq = parse(vec![w("a"), Token::Op(Operator::Or), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Or);
    }

    #[test]
    fn parse_mixed_sequencing_operators() {
        // a && b || c ; d
        let seq = parse(vec![
            w("a"),
            Token::Op(Operator::And),
            w("b"),
            Token::Op(Operator::Or),
            w("c"),
            Token::Op(Operator::Semi),
            w("d"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[])]);
        assert_eq!(
            seq.rest.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            vec![Connector::And, Connector::Or, Connector::Semi]
        );
        assert_eq!(seq.rest[0].1.commands, vec![plain("b", &[])]);
        assert_eq!(seq.rest[1].1.commands, vec![plain("c", &[])]);
        assert_eq!(seq.rest[2].1.commands, vec![plain("d", &[])]);
    }

    #[test]
    fn parse_sequence_of_multi_stage_pipelines() {
        // ls | grep foo && find . -name bar | wc -l
        let seq = parse(vec![
            w("ls"),
            Token::Op(Operator::Pipe),
            w("grep"),
            w("foo"),
            Token::Op(Operator::And),
            w("find"),
            w("."),
            w("-name"),
            w("bar"),
            Token::Op(Operator::Pipe),
            w("wc"),
            w("-l"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            seq.first.commands,
            vec![plain("ls", &[]), plain("grep", &["foo"])]
        );
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::And);
        assert_eq!(
            seq.rest[0].1.commands,
            vec![plain("find", &[".", "-name", "bar"]), plain("wc", &["-l"])]
        );
    }

    #[test]
    fn parse_pipeline_with_redirect_inside_sequence() {
        // echo hi > f ; cat f
        let seq = parse(vec![
            w("echo"),
            w("hi"),
            Token::Op(Operator::RedirOut),
            w("f"),
            Token::Op(Operator::Semi),
            w("cat"),
            w("f"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            seq.first.commands[0].stdout,
            Some(Redirect::Truncate("f".to_string()))
        );
        assert_eq!(seq.rest[0].1.commands, vec![plain("cat", &["f"])]);
    }

    #[test]
    fn parse_trailing_semicolon_is_allowed() {
        let seq = parse(vec![w("a"), Token::Op(Operator::Semi)])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first.commands, vec![plain("a", &[])]);
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_trailing_and_is_missing_command() {
        assert_eq!(
            parse(vec![w("a"), Token::Op(Operator::And)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_trailing_or_is_missing_command() {
        assert_eq!(
            parse(vec![w("a"), Token::Op(Operator::Or)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_leading_semicolon_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Semi), w("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_sequencing_op_is_missing_command() {
        assert_eq!(
            parse(vec![
                w("a"),
                Token::Op(Operator::And),
                Token::Op(Operator::And),
                w("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_target_is_sequencing_op_is_error() {
        // ls > ;
        assert_eq!(
            parse(vec![
                w("ls"),
                Token::Op(Operator::RedirOut),
                Token::Op(Operator::Semi),
            ]),
            Err(ParseError::RedirectTargetIsOperator)
        );
    }
}
```

- [ ] **Step 2: Replace `src/executor.rs` entirely with this**

This renames the existing private multi-stage helper from `run_pipeline` to `run_multi_stage`, demotes the existing public `execute(&Pipeline)` body to a private `run_pipeline(&Pipeline)`, and adds a new public `execute(&Sequence)` with an interim stub. Everything else in the file is unchanged from v2.

```rust
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{Command, Pipeline, Redirect, Sequence};

pub fn execute(seq: &Sequence) -> ExecOutcome {
    // INTERIM (made real in Task 3): only the first pipeline runs; any rest
    // is reported as "not yet implemented".
    if !seq.rest.is_empty() {
        eprintln!("shuck: sequencing not yet implemented");
        return ExecOutcome::Continue(1);
    }
    run_pipeline(&seq.first)
}

fn run_pipeline(pipeline: &Pipeline) -> ExecOutcome {
    if pipeline.commands.len() == 1 {
        run_single(&pipeline.commands[0])
    } else {
        run_multi_stage(&pipeline.commands)
    }
}

// ----- redirect file handling -----------------------------------------------

/// The redirect files a command needs, already opened.
struct StageFiles {
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

/// Opens every redirect file a command needs. On the first failure, prints the
/// error and returns `Err(())` — the caller must then run nothing.
fn open_stage_files(cmd: &Command) -> Result<StageFiles, ()> {
    let stdin = match &cmd.stdin {
        Some(path) => match File::open(path) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("shuck: {path}: {e}");
                return Err(());
            }
        },
        None => None,
    };
    let stdout = match &cmd.stdout {
        Some(redirect) => match open_output(redirect) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("shuck: {}: {e}", redirect_path(redirect));
                return Err(());
            }
        },
        None => None,
    };
    let stderr = match &cmd.stderr {
        Some(redirect) => match open_output(redirect) {
            Ok(file) => Some(file),
            Err(e) => {
                eprintln!("shuck: {}: {e}", redirect_path(redirect));
                return Err(());
            }
        },
        None => None,
    };
    Ok(StageFiles { stdin, stdout, stderr })
}

fn open_output(redirect: &Redirect) -> io::Result<File> {
    match redirect {
        Redirect::Truncate(path) => OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path),
        Redirect::Append(path) => OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .open(path),
    }
}

fn redirect_path(redirect: &Redirect) -> &str {
    match redirect {
        Redirect::Truncate(path) | Redirect::Append(path) => path,
    }
}

/// Maps a finished child's status to an exit code, using the POSIX
/// `128 + signal` convention for signal-killed children.
fn status_code(status: &ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| status.signal().map(|s| 128 + s).unwrap_or(1))
}

// ----- single command -------------------------------------------------------

fn run_single(cmd: &Command) -> ExecOutcome {
    let files = match open_stage_files(cmd) {
        Ok(files) => files,
        Err(()) => return ExecOutcome::Continue(1),
    };

    if builtins::is_builtin(&cmd.program) {
        match files.stdout {
            Some(mut file) => builtins::run_builtin(&cmd.program, &cmd.args, &mut file),
            None => {
                let mut out = io::stdout();
                builtins::run_builtin(&cmd.program, &cmd.args, &mut out)
            }
        }
    } else {
        run_subprocess(cmd, files)
    }
}

fn run_subprocess(cmd: &Command, files: StageFiles) -> ExecOutcome {
    let mut process = ProcessCommand::new(&cmd.program);
    process.args(&cmd.args);
    if let Some(file) = files.stdin {
        process.stdin(Stdio::from(file));
    }
    if let Some(file) = files.stdout {
        process.stdout(Stdio::from(file));
    }
    if let Some(file) = files.stderr {
        process.stderr(Stdio::from(file));
    }

    match process.status() {
        Ok(status) => ExecOutcome::Continue(status_code(&status)),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            eprintln!("shuck: command not found: {}", cmd.program);
            ExecOutcome::Continue(127)
        }
        Err(e) => {
            eprintln!("shuck: {}: {e}", cmd.program);
            ExecOutcome::Continue(1)
        }
    }
}

// ----- multi-stage pipeline -------------------------------------------------

/// What a stage hands to the next stage's stdin.
enum Carry {
    None,
    ChildStdout(ChildStdout),
    Buffer(Vec<u8>),
}

/// A pipeline stage awaiting its final status.
enum Stage {
    Done(i32),
    Process(Child),
}

fn run_multi_stage(commands: &[Command]) -> ExecOutcome {
    // Pre-flight: open every redirect file first. If any fails, run nothing.
    let mut all_files: Vec<StageFiles> = Vec::with_capacity(commands.len());
    for cmd in commands {
        match open_stage_files(cmd) {
            Ok(files) => all_files.push(files),
            Err(()) => return ExecOutcome::Continue(1),
        }
    }

    let n = commands.len();
    let mut stages: Vec<Stage> = Vec::with_capacity(n);
    let mut carry = Carry::None;

    for (i, (cmd, files)) in commands.iter().zip(all_files).enumerate() {
        let is_last = i == n - 1;
        let incoming = std::mem::replace(&mut carry, Carry::None);

        if builtins::is_builtin(&cmd.program) {
            drop(incoming);

            if cmd.program == "cd" || cmd.program == "exit" {
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(0));
                continue;
            }

            let mut buffer: Vec<u8> = Vec::new();
            let outcome = builtins::run_builtin(&cmd.program, &cmd.args, &mut buffer);
            let mut status = match outcome {
                ExecOutcome::Continue(code) => code,
                ExecOutcome::Exit(code) => code,
            };
            match files.stdout {
                Some(mut file) => {
                    if let Err(e) = file.write_all(&buffer) {
                        eprintln!("shuck: {}: {e}", cmd.program);
                        status = 1;
                    }
                    if !is_last {
                        carry = Carry::Buffer(Vec::new());
                    }
                }
                None => {
                    if is_last {
                        if let Err(e) = io::stdout().write_all(&buffer) {
                            eprintln!("shuck: {}: {e}", cmd.program);
                            status = 1;
                        }
                    } else {
                        carry = Carry::Buffer(buffer);
                    }
                }
            }
            stages.push(Stage::Done(status));
            continue;
        }

        let mut process = ProcessCommand::new(&cmd.program);
        process.args(&cmd.args);

        let mut pending_input: Option<Vec<u8>> = None;
        if let Some(file) = files.stdin {
            process.stdin(Stdio::from(file));
        } else {
            match incoming {
                Carry::None => {}
                Carry::ChildStdout(child_stdout) => {
                    process.stdin(Stdio::from(child_stdout));
                }
                Carry::Buffer(bytes) => {
                    process.stdin(Stdio::piped());
                    pending_input = Some(bytes);
                }
            }
        }

        let pipe_onward = !is_last && cmd.stdout.is_none();
        if let Some(file) = files.stdout {
            process.stdout(Stdio::from(file));
        } else if pipe_onward {
            process.stdout(Stdio::piped());
        }

        if let Some(file) = files.stderr {
            process.stderr(Stdio::from(file));
        }

        let mut child = match process.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                eprintln!("shuck: command not found: {}", cmd.program);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(127));
                continue;
            }
            Err(e) => {
                eprintln!("shuck: {}: {e}", cmd.program);
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(1));
                continue;
            }
        };

        if let Some(bytes) = pending_input {
            if let Some(mut child_stdin) = child.stdin.take() {
                let _ = child_stdin.write_all(&bytes);
            }
        }

        if pipe_onward {
            carry = Carry::ChildStdout(child.stdout.take().expect("stdout was set to piped"));
        } else if !is_last {
            carry = Carry::Buffer(Vec::new());
        }

        stages.push(Stage::Process(child));
    }

    let mut last_status = 0;
    for stage in stages {
        match stage {
            Stage::Done(code) => last_status = code,
            Stage::Process(mut child) => {
                last_status = match child.wait() {
                    Ok(status) => status_code(&status),
                    Err(e) => {
                        eprintln!("shuck: {e}");
                        1
                    }
                };
            }
        }
    }
    ExecOutcome::Continue(last_status)
}
```

- [ ] **Step 3: Update `src/shell.rs`** (one tiny edit)

Replace this line in `process_line`:

```rust
        Ok(Some(pipeline)) => executor::execute(&pipeline),
```

with:

```rust
        Ok(Some(sequence)) => executor::execute(&sequence),
```

(`executor::execute` now takes `&Sequence`; the variable rename keeps things readable.)

- [ ] **Step 4: Verify the crate builds and all tests pass**

Run: `cargo build`
Expected: PASS — no warnings.

Run: `cargo test`
Expected: PASS — 76 tests pass (38 lexer + 31 command + 7 builtins). The interim Task-1 command test was replaced by the real sequencing tests in Step 1.

- [ ] **Step 5: Commit**

```bash
git add src/command.rs src/executor.rs src/shell.rs
git commit -m "feat: parse Sequence of pipelines; wire executor and shell"
```

---

## Task 3: Execute sequences with short-circuit semantics

Replace the interim stub in `execute(&Sequence)` with the real left-to-right loop. `Exit` from any pipeline short-circuits the rest. The executor has no unit tests (it touches real processes); verification is build + existing tests + a manual smoke test.

**Files:**
- Modify: `src/executor.rs` (two small edits)

- [ ] **Step 1: Update imports in `src/executor.rs`**

Replace:

```rust
use crate::command::{Command, Pipeline, Redirect, Sequence};
```

with:

```rust
use crate::command::{Command, Connector, Pipeline, Redirect, Sequence};
```

- [ ] **Step 2: Replace the body of `pub fn execute`**

Replace this block:

```rust
pub fn execute(seq: &Sequence) -> ExecOutcome {
    // INTERIM (made real in Task 3): only the first pipeline runs; any rest
    // is reported as "not yet implemented".
    if !seq.rest.is_empty() {
        eprintln!("shuck: sequencing not yet implemented");
        return ExecOutcome::Continue(1);
    }
    run_pipeline(&seq.first)
}
```

with:

```rust
pub fn execute(seq: &Sequence) -> ExecOutcome {
    let mut status = run_pipeline(&seq.first);
    if matches!(status, ExecOutcome::Exit(_)) {
        return status;
    }
    for (connector, pipeline) in &seq.rest {
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_pipeline(pipeline);
            if matches!(status, ExecOutcome::Exit(_)) {
                return status;
            }
        }
    }
    status
}
```

- [ ] **Step 3: Verify the crate builds and all tests pass**

Run: `cargo build`
Expected: PASS — no warnings.

Run: `cargo test`
Expected: PASS — 76 tests pass (no test count change; the executor has no unit tests).

- [ ] **Step 4: Manual smoke test of sequencing**

Run this and confirm:

```bash
cargo build -q
printf '%s\n' \
  'true && echo a' \
  'false && echo b' \
  'true || echo c' \
  'false || echo d' \
  'echo e ; echo f' \
  'false || echo g | tr a-z A-Z' \
  'true && echo h && echo i' \
  'true && false || echo j' \
  'exit 0' \
  | ./target/debug/shuck
```

Expected output:
```
a
d
e
f
G
h
i
j
```
Explanation: `true && echo a` → `a` (true succeeded). `false && echo b` → silent (false short-circuits `&&`). `true || echo c` → silent (true short-circuits `||`). `false || echo d` → `d`. `echo e ; echo f` → both. `false || echo g | tr a-z A-Z` → `||` runs the next pipeline `echo g | tr a-z A-Z` → `G`. `true && echo h && echo i` → both `h` and `i`. `true && false || echo j` → `true` succeeds, runs `false` (status 1), `||` runs `echo j` → `j`.

- [ ] **Step 5: Commit**

```bash
git add src/executor.rs
git commit -m "feat: execute sequences with short-circuit semantics"
```

---

## Task 4: Full smoke test

End-to-end verification of sequencing combined with pipes and redirection. This task is verification only — no code changes, no commit.

**Files:** none

- [ ] **Step 1: Run the combined smoke script**

```bash
cargo build -q
W=$(mktemp -d)
printf '%s\n' \
  'true && echo basic-and' \
  'false || echo basic-or' \
  'echo one ; echo two' \
  "ls /nonexistent-xyz && echo never-printed || echo recovered" \
  "echo hi > $W/out && cat < $W/out" \
  "echo a | grep a && echo grep-matched" \
  "echo a | grep zzz || echo grep-missed" \
  'echo trail ;' \
  'exit 0' \
  | ./target/debug/shuck
rm -rf "$W"
```

Expected output (stderr and stdout will interleave):
```
basic-and
basic-or
one
two
ls: cannot access '/nonexistent-xyz': No such file or directory
recovered
hi
grep-matched
grep-missed
trail
```
Explanation: `ls /nonexistent-xyz && ...` → `ls` prints its error to stderr and exits non-zero; `&&` skips `echo never-printed`; `||` then runs `echo recovered`. `echo hi > out && cat < out` → redirect-aware sequencing. `grep` matching governs the next connector. Trailing `;` is accepted (echoes `trail` only).

- [ ] **Step 2: Verify exit-status propagation and syntax errors**

```bash
printf 'true && false ; true\n'  | ./target/debug/shuck; echo "exit=$?"
printf 'true && false\n'         | ./target/debug/shuck; echo "exit=$?"
printf 'a && && b\n'             | ./target/debug/shuck
printf '; ls\n'                  | ./target/debug/shuck
printf 'ls &\n'                  | ./target/debug/shuck
printf 'echo "&&"\n'             | ./target/debug/shuck
```

Expected:
```
exit=0
exit=1
shuck: syntax error: expected a command
shuck: syntax error: expected a command
shuck: syntax error: unexpected '&'
&&
```
Explanation: `true && false ; true` → last pipeline `true` → exit 0. `true && false` → last pipeline `false` → exit 1. `a && && b`, `; ls` → `MissingCommand`. `ls &` → bare-ampersand lex error. `echo "&&"` → quoted operator is a literal word.

- [ ] **Step 3: Confirm**

Both step outputs match exactly. If any line differs, stop and fix the relevant module before completing the plan.

---

## Self-Review Notes

- **Spec coverage:** lexer operators `And`/`Or`/`Semi` with the documented lookahead rules and `BareAmpersand` error (Task 1); `Connector` and `Sequence` types (Task 2); `parse_pipeline` extracted as private helper, `parse` returns `Result<Option<Sequence>, ParseError>` (Task 2); trailing `;` allowed via the explicit branch in the outer parse loop (Task 2 test `parse_trailing_semicolon_is_allowed`); leading/trailing `&&`/`||` and double sequencing op (Task 2 tests); redirect target as sequencing op (Task 2 test `parse_redirect_target_is_sequencing_op_is_error`); multi-stage pipelines on each side of `&&` covered by Task 2's `parse_sequence_of_multi_stage_pipelines`; pipeline-with-redirect inside a sequence (Task 2); executor rename to `run_pipeline`/`run_multi_stage` and new `execute(&Sequence)` (Task 2); short-circuit loop with `Exit` propagation (Task 3); shell `process_line` updates including `lex_error_message` for `BareAmpersand` (Task 1).
- **Type consistency:** `Operator` variants `And`/`Or`/`Semi` are introduced in Task 1 and consumed by Tasks 1 (interim) and 2 (real). `Connector` (`Semi`/`And`/`Or`) is defined in Task 2's `command.rs` and consumed in Task 3's `execute` body. `Sequence { first: Pipeline, rest: Vec<(Connector, Pipeline)> }` is defined in Task 2 and consumed by `executor::execute` (Task 2 interim, Task 3 real) and by shell.rs (Task 2 variable rename). `parse_pipeline` is private — only `command.rs`'s `parse` calls it. `run_pipeline`/`run_multi_stage`/`run_single` are private — only `execute` calls them. `LexError::BareAmpersand` is defined in Task 1 and handled in shell.rs via `lex_error_message` (Task 1).
- **Interim states are explicit:** Task 1's `command.rs` carries an `INTERIM` comment on the new sequencing arm and a `Task 2 deletes this` note on the throwaway test. Task 2's `executor.rs` `execute` body carries an `INTERIM` comment. Each task leaves the crate compiling with all unit tests green.
- **No placeholders:** every code step is a complete file or a complete, located edit; every run step has an exact command and expected output.
