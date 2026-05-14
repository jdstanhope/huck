# shuck Pipes and Redirection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `|` pipelines and `>`, `>>`, `<`, `2>`, `2>>` redirection to the `shuck` shell.

**Architecture:** The data flow grows from `&str -> Vec<String> -> Command -> ExecOutcome` to `&str -> Vec<Token> -> Pipeline -> ExecOutcome`. The lexer emits a `Token` enum (`Word` or `Op`), the parser builds a `Pipeline` of redirect-aware `Command`s, and the executor wires up `Stdio` between stages. Builtins `echo`/`pwd` are refactored to write to an injectable `&mut dyn Write` so their output can be redirected or piped.

**Tech Stack:** Rust (edition 2024), `std::process` (`Command`, `Stdio`, `Child`), `std::fs` (`File`, `OpenOptions`). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-14-shuck-pipes-redirection-design.md`

---

## File Structure

| File | Change |
|------|--------|
| `src/builtins.rs` | `echo`/`pwd` and `run_builtin` take `&mut dyn Write`; add `echo`/`pwd` unit tests. |
| `src/lexer.rs` | `tokenize` returns `Vec<Token>` (`Word` \| `Op(Operator)`); operator recognition. |
| `src/command.rs` | New `Redirect`, `Pipeline`, `ParseError`; `Command` gains redirect fields; `parse` returns `Result<Option<Pipeline>, ParseError>`. |
| `src/executor.rs` | `execute` takes `&Pipeline`; opens redirect files, applies `Stdio`, runs multi-stage pipelines. |
| `src/shell.rs` | `process_line` handles the new `Result<Option<Pipeline>, ParseError>`. |
| `src/main.rs` | Unchanged. |

**Why the task order:** This is an interface migration that touches every module. Each task leaves the crate compiling and all unit tests green. Tasks 2 and 3 use deliberately-marked **interim** code in downstream modules so the crate stays buildable; Tasks 4 and 5 replace the interim executor with the real implementation. Per-task verification is `cargo test` (a binary-only crate — never `cargo test --lib`).

---

## Task 1: Builtins write to an injectable writer

`echo` and `pwd` currently use `println!`, which hard-wires their output to the terminal. Refactor them (and `run_builtin`) to take a `&mut dyn Write` so the executor can later point that output at a file or a pipe. Behavior is identical; this also makes `echo`/`pwd` unit-testable.

**Files:**
- Modify: `src/builtins.rs` (full replacement below)
- Modify: `src/executor.rs` (call-site update)

- [ ] **Step 1: Replace `src/builtins.rs` entirely with this**

```rust
use std::env;
use std::io::Write;
use std::path::Path;

/// The result of running a command — either the shell continues (carrying the
/// command's exit status) or the shell should terminate with a code.
#[derive(Debug)]
pub enum ExecOutcome {
    Continue(i32),
    Exit(i32),
}

pub fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "exit" | "pwd" | "echo")
}

/// Runs a builtin. Caller must ensure `is_builtin(name)` is true. `out` is the
/// destination for any stdout the builtin produces (`echo`, `pwd`); `cd` and
/// `exit` produce no stdout and ignore it.
pub fn run_builtin(name: &str, args: &[String], out: &mut dyn Write) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
        "exit" => builtin_exit(args),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}

fn builtin_cd(args: &[String]) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("shuck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }
    let target = match args.first() {
        Some(dir) => dir.clone(),
        None => match env::var("HOME") {
            Ok(home) => home,
            Err(_) => {
                eprintln!("shuck: cd: HOME not set");
                return ExecOutcome::Continue(1);
            }
        },
    };
    match env::set_current_dir(Path::new(&target)) {
        Ok(()) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("shuck: cd: {target}: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_pwd(out: &mut dyn Write) -> ExecOutcome {
    match env::current_dir() {
        Ok(path) => {
            if let Err(e) = writeln!(out, "{}", path.display()) {
                eprintln!("shuck: pwd: {e}");
                return ExecOutcome::Continue(1);
            }
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            eprintln!("shuck: pwd: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_echo(args: &[String], out: &mut dyn Write) -> ExecOutcome {
    if let Err(e) = writeln!(out, "{}", args.join(" ")) {
        eprintln!("shuck: echo: {e}");
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
}

fn builtin_exit(args: &[String]) -> ExecOutcome {
    match args.first() {
        None => ExecOutcome::Exit(0),
        Some(code_str) => match code_str.parse::<i32>() {
            Ok(code) => ExecOutcome::Exit(code),
            Err(_) => {
                eprintln!("shuck: exit: {code_str}: numeric argument required");
                ExecOutcome::Continue(2)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_builtin_recognizes_builtins() {
        assert!(is_builtin("cd"));
        assert!(is_builtin("exit"));
        assert!(is_builtin("pwd"));
        assert!(is_builtin("echo"));
        assert!(!is_builtin("ls"));
    }

    #[test]
    fn exit_with_no_args() {
        assert!(matches!(builtin_exit(&[]), ExecOutcome::Exit(0)));
    }

    #[test]
    fn exit_with_code() {
        assert!(matches!(
            builtin_exit(&["3".to_string()]),
            ExecOutcome::Exit(3)
        ));
    }

    #[test]
    fn exit_with_bad_code_continues() {
        assert!(matches!(
            builtin_exit(&["abc".to_string()]),
            ExecOutcome::Continue(_)
        ));
    }

    #[test]
    fn echo_writes_args_joined_by_spaces() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_echo(&["hello".to_string(), "world".to_string()], &mut out);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(out, b"hello world\n");
    }

    #[test]
    fn echo_with_no_args_writes_a_blank_line() {
        let mut out: Vec<u8> = Vec::new();
        builtin_echo(&[], &mut out);
        assert_eq!(out, b"\n");
    }

    #[test]
    fn pwd_writes_the_current_directory() {
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_pwd(&mut out);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let written = String::from_utf8(out).unwrap();
        let expected = env::current_dir().unwrap();
        assert_eq!(written.trim_end(), expected.to_str().unwrap());
    }
}
```

- [ ] **Step 2: Update the `run_builtin` call site in `src/executor.rs`**

The current `src/executor.rs` calls `run_builtin` with two arguments. Change its imports and that call site. Replace the import line:

```rust
use std::io::ErrorKind;
```

with:

```rust
use std::io::{self, ErrorKind};
```

and replace this block:

```rust
    if builtins::is_builtin(&cmd.program) {
        return builtins::run_builtin(&cmd.program, &cmd.args);
    }
```

with:

```rust
    if builtins::is_builtin(&cmd.program) {
        let mut out = io::stdout();
        return builtins::run_builtin(&cmd.program, &cmd.args, &mut out);
    }
```

- [ ] **Step 3: Verify the crate builds and all tests pass**

Run: `cargo build`
Expected: PASS — no warnings.

Run: `cargo test`
Expected: PASS — 23 tests pass (13 lexer + 3 command + 7 builtins).

- [ ] **Step 4: Commit**

```bash
git add src/builtins.rs src/executor.rs
git commit -m "refactor: builtins write output to an injectable writer"
```

---

## Task 2: Lexer emits operator tokens

Change `tokenize` to return `Vec<Token>` (a `Word` or an `Op`), recognizing `|`, `<`, `>`, `>>`, `2>`, `2>>`. To keep the crate compiling, `command.rs` gets a deliberately-**interim** `parse` that handles only word tokens (a line with any operator parses to `None` for now); Task 3 replaces it with the real pipeline parser.

**Files:**
- Modify: `src/lexer.rs` (full replacement below)
- Modify: `src/command.rs` (interim replacement below)

- [ ] **Step 1: Replace `src/lexer.rs` entirely with this**

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Operator {
    Pipe,           // |
    RedirOut,       // >
    RedirAppend,    // >>
    RedirIn,        // <
    RedirErr,       // 2>
    RedirErrAppend, // 2>>
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
                tokens.push(Token::Op(Operator::Pipe));
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
}
```

- [ ] **Step 2: Replace `src/command.rs` entirely with this interim version**

```rust
use crate::lexer::Token;

#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

/// INTERIM (replaced by the full pipeline parser in Task 3): builds a single
/// command from word tokens. A line containing any operator token is not yet
/// supported and parses to `None`, so the REPL simply re-prompts.
pub fn parse(tokens: Vec<Token>) -> Option<Command> {
    let mut words = Vec::new();
    for token in tokens {
        match token {
            Token::Word(w) => words.push(w),
            Token::Op(_) => return None,
        }
    }
    let mut iter = words.into_iter();
    let program = iter.next()?;
    let args = iter.collect();
    Some(Command { program, args })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Operator;

    fn w(s: &str) -> Token {
        Token::Word(s.to_string())
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), None);
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w("ls")]),
            Some(Command {
                program: "ls".to_string(),
                args: vec![],
            })
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w("ls"), w("-la"), w("/tmp")]),
            Some(Command {
                program: "ls".to_string(),
                args: vec!["-la".to_string(), "/tmp".to_string()],
            })
        );
    }

    #[test]
    fn parse_operator_token_returns_none_for_now() {
        assert_eq!(
            parse(vec![w("ls"), Token::Op(Operator::Pipe), w("cat")]),
            None
        );
    }
}
```

- [ ] **Step 3: Verify the crate builds and all tests pass**

Run: `cargo build`
Expected: PASS — no warnings. (`shell.rs` and `executor.rs` are untouched and still compile: `parse` still returns `Option<Command>` and `execute` still takes `&Command`.)

Run: `cargo test`
Expected: PASS — 37 tests pass (26 lexer + 4 command + 7 builtins).

- [ ] **Step 4: Commit**

```bash
git add src/lexer.rs src/command.rs
git commit -m "feat: lexer emits operator tokens"
```

---

## Task 3: Parse pipelines and redirections

Replace the interim parser with the real one: `parse` builds a `Pipeline` of redirect-aware `Command`s and returns `Result<Option<Pipeline>, ParseError>`. `shell.rs` is updated to surface parse errors. `executor.rs` becomes an **interim** version that runs a single command without redirections (Tasks 4 and 5 make it real).

**Files:**
- Modify: `src/command.rs` (full replacement below)
- Modify: `src/shell.rs` (edits below)
- Modify: `src/executor.rs` (interim replacement below)

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

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Pipeline>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }

    let mut commands: Vec<Command> = Vec::new();

    // Builder state for the command currently being assembled.
    let mut program: Option<String> = None;
    let mut args: Vec<String> = Vec::new();
    let mut stdin: Option<String> = None;
    let mut stdout: Option<Redirect> = None;
    let mut stderr: Option<Redirect> = None;

    let mut iter = tokens.into_iter();

    while let Some(token) = iter.next() {
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
                    Operator::Pipe => unreachable!("Pipe is handled in the arm above"),
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

    Ok(Some(Pipeline { commands }))
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

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), Ok(None));
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w("ls")]),
            Ok(Some(Pipeline {
                commands: vec![plain("ls", &[])],
            }))
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w("ls"), w("-la"), w("/tmp")]),
            Ok(Some(Pipeline {
                commands: vec![plain("ls", &["-la", "/tmp"])],
            }))
        );
    }

    #[test]
    fn parse_redirect_out() {
        let pipeline = parse(vec![w("ls"), Token::Op(Operator::RedirOut), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(
            pipeline.commands[0].stdout,
            Some(Redirect::Truncate("f".to_string()))
        );
    }

    #[test]
    fn parse_redirect_append() {
        let pipeline = parse(vec![w("ls"), Token::Op(Operator::RedirAppend), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(
            pipeline.commands[0].stdout,
            Some(Redirect::Append("f".to_string()))
        );
    }

    #[test]
    fn parse_redirect_in() {
        let pipeline = parse(vec![w("cat"), Token::Op(Operator::RedirIn), w("f")])
            .unwrap()
            .unwrap();
        assert_eq!(pipeline.commands[0].stdin, Some("f".to_string()));
    }

    #[test]
    fn parse_redirect_stderr() {
        let pipeline = parse(vec![w("cmd"), Token::Op(Operator::RedirErr), w("e")])
            .unwrap()
            .unwrap();
        assert_eq!(
            pipeline.commands[0].stderr,
            Some(Redirect::Truncate("e".to_string()))
        );
    }

    #[test]
    fn parse_redirect_stderr_append() {
        let pipeline = parse(vec![w("cmd"), Token::Op(Operator::RedirErrAppend), w("e")])
            .unwrap()
            .unwrap();
        assert_eq!(
            pipeline.commands[0].stderr,
            Some(Redirect::Append("e".to_string()))
        );
    }

    #[test]
    fn parse_two_stage_pipeline() {
        let pipeline = parse(vec![w("a"), Token::Op(Operator::Pipe), w("b")])
            .unwrap()
            .unwrap();
        assert_eq!(pipeline.commands, vec![plain("a", &[]), plain("b", &[])]);
    }

    #[test]
    fn parse_three_stage_pipeline() {
        let pipeline = parse(vec![
            w("a"),
            Token::Op(Operator::Pipe),
            w("b"),
            Token::Op(Operator::Pipe),
            w("c"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(pipeline.commands.len(), 3);
    }

    #[test]
    fn parse_pipeline_with_redirects_on_stages() {
        // a < in | b > out
        let pipeline = parse(vec![
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
        assert_eq!(pipeline.commands[0].stdin, Some("in".to_string()));
        assert_eq!(
            pipeline.commands[1].stdout,
            Some(Redirect::Truncate("out".to_string()))
        );
    }

    #[test]
    fn parse_last_redirect_of_a_kind_wins() {
        // ls > a > b
        let pipeline = parse(vec![
            w("ls"),
            Token::Op(Operator::RedirOut),
            w("a"),
            Token::Op(Operator::RedirOut),
            w("b"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            pipeline.commands[0].stdout,
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
}
```

- [ ] **Step 2: Update `src/shell.rs`**

Replace the import line:

```rust
use crate::command;
```

with:

```rust
use crate::command::{self, ParseError};
```

Replace the body of `process_line` — specifically this block:

```rust
    match command::parse(tokens) {
        Some(cmd) => executor::execute(&cmd),
        None => ExecOutcome::Continue(0),
    }
}
```

with:

```rust
    match command::parse(tokens) {
        Ok(Some(pipeline)) => executor::execute(&pipeline),
        Ok(None) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("shuck: syntax error: {}", parse_error_message(e));
            ExecOutcome::Continue(2)
        }
    }
}

fn parse_error_message(error: ParseError) -> &'static str {
    match error {
        ParseError::MissingCommand => "expected a command",
        ParseError::MissingRedirectTarget => "expected a filename after redirection",
        ParseError::RedirectTargetIsOperator => "expected a filename after redirection",
    }
}
```

- [ ] **Step 3: Replace `src/executor.rs` entirely with this interim version**

```rust
use std::io::{self, ErrorKind};
use std::os::unix::process::ExitStatusExt;
use std::process::Command as ProcessCommand;

use crate::builtins::{self, ExecOutcome};
use crate::command::{Command, Pipeline};

pub fn execute(pipeline: &Pipeline) -> ExecOutcome {
    // INTERIM: redirection (Task 4) and multi-command pipelines (Task 5) are
    // wired up in the following tasks. For now only a single command with no
    // redirections runs.
    if pipeline.commands.len() > 1 {
        eprintln!("shuck: pipelines not yet implemented");
        return ExecOutcome::Continue(1);
    }
    let cmd = &pipeline.commands[0];
    if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
        eprintln!("shuck: redirection not yet implemented");
        return ExecOutcome::Continue(1);
    }
    run_simple(cmd)
}

fn run_simple(cmd: &Command) -> ExecOutcome {
    if builtins::is_builtin(&cmd.program) {
        let mut out = io::stdout();
        return builtins::run_builtin(&cmd.program, &cmd.args, &mut out);
    }

    match ProcessCommand::new(&cmd.program).args(&cmd.args).status() {
        Ok(status) => {
            let code = status
                .code()
                .unwrap_or_else(|| status.signal().map(|s| 128 + s).unwrap_or(1));
            ExecOutcome::Continue(code)
        }
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
```

- [ ] **Step 4: Verify the crate builds and all tests pass**

Run: `cargo build`
Expected: PASS — no warnings.

Run: `cargo test`
Expected: PASS — 51 tests pass (26 lexer + 18 command + 7 builtins).

- [ ] **Step 5: Commit**

```bash
git add src/command.rs src/shell.rs src/executor.rs
git commit -m "feat: parse pipelines and redirections"
```

---

## Task 4: Apply redirections to single commands

Replace the interim executor with one that opens redirect files and applies them to a single command — both subprocesses (via `Stdio`) and builtins (via the injectable writer). Multi-command pipelines still report "not yet implemented" — Task 5 handles them. The executor has no unit tests (it touches real processes and files); verification is a build, the existing test suite, and a manual smoke test.

**Files:**
- Modify: `src/executor.rs` (full replacement below)

- [ ] **Step 1: Replace `src/executor.rs` entirely with this**

```rust
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{Command, Pipeline, Redirect};

pub fn execute(pipeline: &Pipeline) -> ExecOutcome {
    // INTERIM: multi-command pipelines are wired up in Task 5.
    if pipeline.commands.len() > 1 {
        eprintln!("shuck: pipelines not yet implemented");
        return ExecOutcome::Continue(1);
    }
    run_single(&pipeline.commands[0])
}

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

fn run_single(cmd: &Command) -> ExecOutcome {
    let files = match open_stage_files(cmd) {
        Ok(files) => files,
        Err(()) => return ExecOutcome::Continue(1),
    };

    if builtins::is_builtin(&cmd.program) {
        // Builtins do not read stdin; an opened `<` or `2>` file is ignored
        // (it is still opened above, so a bad path is still reported).
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
```

- [ ] **Step 2: Verify the crate builds and all tests pass**

Run: `cargo build`
Expected: PASS — no warnings.

Run: `cargo test`
Expected: PASS — 51 tests pass (unchanged from Task 3; the executor has no unit tests).

- [ ] **Step 3: Manual smoke test of single-command redirections**

Run this and confirm each line:

```bash
cargo build -q
W=$(mktemp -d)
printf '%s\n' \
  "echo hello > $W/out" \
  "cat $W/out" \
  "echo world >> $W/out" \
  "cat < $W/out" \
  "ls /nonexistent-xyz 2> $W/err" \
  "cat $W/err" \
  "cat < /nonexistent/path" \
  "exit 0" \
  | ./target/debug/shuck
rm -rf "$W"
```

Expected output (stdout + stderr interleaved):
```
hello
hello
world
ls: cannot access '/nonexistent-xyz': No such file or directory
shuck: /nonexistent/path: No such file or directory (os error 2)
```
(`echo ... > out` and `echo ... >> out` print nothing; `cat $W/out` after the append shows `hello` then `world`; the `ls` error text is captured into `$W/err` and printed by `cat $W/err`; the bad `<` path prints a `shuck:` error and the shell continues.)

- [ ] **Step 4: Commit**

```bash
git add src/executor.rs
git commit -m "feat: apply redirections to single commands"
```

---

## Task 5: Execute multi-command pipelines

Replace the interim executor with the full one: `execute` routes single commands to `run_single` (from Task 4, unchanged) and multi-command pipelines to a new `run_pipeline`. Subprocess stages are chained with `Stdio` pipes; builtin stages (`echo`/`pwd`) write to an in-memory buffer that feeds the next stage; `cd`/`exit` inside a pipeline are no-ops. The pipeline's exit status is the last stage's status.

**Files:**
- Modify: `src/executor.rs` (full replacement below)

- [ ] **Step 1: Replace `src/executor.rs` entirely with this**

```rust
use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, ChildStdout, Command as ProcessCommand, ExitStatus, Stdio};

use crate::builtins::{self, ExecOutcome};
use crate::command::{Command, Pipeline, Redirect};

pub fn execute(pipeline: &Pipeline) -> ExecOutcome {
    if pipeline.commands.len() == 1 {
        run_single(&pipeline.commands[0])
    } else {
        run_pipeline(&pipeline.commands)
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
        // Builtins do not read stdin; an opened `<` or `2>` file is ignored
        // (it is still opened above, so a bad path is still reported).
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

// ----- multi-command pipeline -----------------------------------------------

/// What a stage hands to the next stage's stdin.
enum Carry {
    /// First stage, or the previous stage produced nothing routable.
    None,
    /// A subprocess stage's piped stdout.
    ChildStdout(ChildStdout),
    /// A builtin stage's captured output (always small).
    Buffer(Vec<u8>),
}

/// A pipeline stage awaiting its final status.
enum Stage {
    /// A builtin (or a failed spawn) that already has a status.
    Done(i32),
    /// A subprocess still to be waited on.
    Process(Child),
}

fn run_pipeline(commands: &[Command]) -> ExecOutcome {
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
            // Builtins do not read stdin; whatever came in is dropped here.
            drop(incoming);

            if cmd.program == "cd" || cmd.program == "exit" {
                // No-op inside a pipeline: shell-state effects are dropped.
                if !is_last {
                    carry = Carry::Buffer(Vec::new());
                }
                stages.push(Stage::Done(0));
                continue;
            }

            // echo / pwd: capture output into a buffer.
            let mut buffer: Vec<u8> = Vec::new();
            let outcome = builtins::run_builtin(&cmd.program, &cmd.args, &mut buffer);
            let status = match outcome {
                ExecOutcome::Continue(code) => code,
                ExecOutcome::Exit(code) => code,
            };
            match files.stdout {
                Some(mut file) => {
                    let _ = file.write_all(&buffer);
                    if !is_last {
                        carry = Carry::Buffer(Vec::new());
                    }
                }
                None => {
                    if is_last {
                        let _ = io::stdout().write_all(&buffer);
                    } else {
                        carry = Carry::Buffer(buffer);
                    }
                }
            }
            stages.push(Stage::Done(status));
            continue;
        }

        // Subprocess stage.
        let mut process = ProcessCommand::new(&cmd.program);
        process.args(&cmd.args);

        // stdin: an explicit `<` wins; otherwise the previous stage's output.
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

        // stdout: an explicit `>` wins; otherwise pipe onward unless last.
        let pipe_onward = !is_last && cmd.stdout.is_none();
        if let Some(file) = files.stdout {
            process.stdout(Stdio::from(file));
        } else if pipe_onward {
            process.stdout(Stdio::piped());
        }

        // stderr: an explicit `2>` wins; otherwise inherit.
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

        // Feed a preceding builtin's buffered output into this child's stdin.
        if let Some(bytes) = pending_input {
            if let Some(mut child_stdin) = child.stdin.take() {
                let _ = child_stdin.write_all(&bytes);
                // `child_stdin` drops here, closing the pipe so the child sees EOF.
            }
        }

        if pipe_onward {
            carry = Carry::ChildStdout(child.stdout.take().expect("stdout was set to piped"));
        } else if !is_last {
            // This stage redirected its stdout to a file, so the next stage
            // has no upstream data — hand it an empty input.
            carry = Carry::Buffer(Vec::new());
        }

        stages.push(Stage::Process(child));
    }

    // Every stage is spawned and all pipes are connected; now wait in spawn
    // order. The pipeline's status is the last stage's status.
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

- [ ] **Step 2: Verify the crate builds and all tests pass**

Run: `cargo build`
Expected: PASS — no warnings.

Run: `cargo test`
Expected: PASS — 51 tests pass (the executor still has no unit tests).

- [ ] **Step 3: Manual smoke test of pipelines**

Run this and confirm:

```bash
cargo build -q
printf '%s\n' \
  'echo piped | tr a-z A-Z' \
  'printf "c\nb\na\n" | sort' \
  'echo hi | nonexistent-cmd-xyz | cat' \
  'echo standalone' \
  'exit 0' \
  | ./target/debug/shuck
```

Expected output:
```
PIPED
a
b
c
shuck: command not found: nonexistent-cmd-xyz
standalone
```
(`echo piped | tr a-z A-Z` → builtin output piped to a subprocess. `printf ... | sort` → two subprocesses chained. The not-found middle stage prints its error; `cat` receives empty input and prints nothing; the shell continues.)

- [ ] **Step 4: Commit**

```bash
git add src/executor.rs
git commit -m "feat: execute multi-command pipelines"
```

---

## Task 6: Full smoke test

The whole feature is exercised through piped stdin (`rustyline` reads piped input fine, as the v1 shell verified). This task is verification only — no code changes, no commit.

**Files:** none

- [ ] **Step 1: Run the combined smoke script**

```bash
cargo build -q
W=$(mktemp -d)
printf '%s\n' \
  "echo hello > $W/out" \
  "cat < $W/out" \
  "echo world >> $W/out" \
  "cat $W/out" \
  'echo piped | tr a-z A-Z' \
  'printf "c\nb\na\n" | sort' \
  "ls /nonexistent-xyz 2> $W/err" \
  "cat $W/err" \
  'echo "a|b"' \
  'cat < /nonexistent/path' \
  'echo hi | nonexistent-cmd-xyz | cat' \
  "cat < $W/out | sort > $W/sorted" \
  "cat $W/sorted" \
  'echo done' \
  'exit 0' \
  | ./target/debug/shuck
rm -rf "$W"
```

Expected output (order matters; the two `shuck:` lines are on stderr but interleave here):
```
hello
hello
world
PIPED
a
b
c
ls: cannot access '/nonexistent-xyz': No such file or directory
a|b
shuck: /nonexistent/path: No such file or directory (os error 2)
shuck: command not found: nonexistent-cmd-xyz
hello
world
done
```

- [ ] **Step 2: Verify pipeline exit status and syntax errors**

```bash
printf 'true | false\n'  | ./target/debug/shuck; echo "exit=$?"
printf 'false | true\n'  | ./target/debug/shuck; echo "exit=$?"
printf 'ls |\n'          | ./target/debug/shuck
printf 'ls >\n'          | ./target/debug/shuck
printf 'ls > | cat\n'    | ./target/debug/shuck
```

Expected:
```
exit=1
exit=0
shuck: syntax error: expected a command
shuck: syntax error: expected a filename after redirection
shuck: syntax error: expected a filename after redirection
```
(The pipeline's status is its last command's: `true | false` → 1, `false | true` → 0, surfaced as the shell's EOF exit code.)

- [ ] **Step 3: Confirm**

All output matches. If any line differs, stop and fix the relevant module before completing the plan.

---

## Self-Review Notes

- **Spec coverage:** operators `|`/`>`/`>>`/`<`/`2>`/`2>>` (Task 2 lexer, Task 3 parser); operators need no surrounding whitespace (Task 2, `tokenize_redirect_out_without_spaces`); quoted/escaped operators stay words (Task 2); `Token` enum (Task 2); `Redirect`/`Command`/`Pipeline`/`ParseError` (Task 3); single-command redirection incl. builtins (Task 4); multi-command pipelines, builtin buffering, `cd`/`exit` no-op in pipeline, last-command exit status (Task 5); redirect-open failure runs nothing (Task 4 `run_single`, Task 5 `run_pipeline` pre-flight); `process_line` error surfacing (Task 3 shell.rs); builtins write to `&mut dyn Write` (Task 1). All spec sections map to a task.
- **Type consistency:** `tokenize -> Result<Vec<Token>, LexError>` (Task 2) is consumed by `parse(Vec<Token>) -> Result<Option<Pipeline>, ParseError>` (Task 3). `Pipeline { commands: Vec<Command> }` and `Command { program, args, stdin, stdout, stderr }` (Task 3) are consumed by `execute(&Pipeline)` (Tasks 3–5). `run_builtin(name, args, &mut dyn Write)` (Task 1) is called by the executor in Tasks 3, 4, 5. `Operator` variants (`Pipe`, `RedirOut`, `RedirAppend`, `RedirIn`, `RedirErr`, `RedirErrAppend`) are consistent between lexer and parser. `StageFiles`, `open_stage_files`, `open_output`, `redirect_path`, `status_code` are defined in Task 4 and reused unchanged in Task 5.
- **Interim states are explicit:** Task 2's `command.rs` and Task 3's `executor.rs` carry `INTERIM` comments and emit honest "not yet implemented" messages rather than silently mis-handling operators or redirects. Each task leaves the crate compiling with all unit tests green.
- **No placeholders:** every code step is a complete file or a complete, located edit; every run step has an exact command and expected output.
