# shuck Shell v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an interactive command-line shell in Rust with four builtin commands (`cd`, `exit`, `pwd`, `echo`) and the ability to run external programs as subprocesses.

**Architecture:** A modular crate where each module has one responsibility — `lexer` (tokenize a line, quote/escape aware), `command` (parse tokens into a `Command`), `builtins` (the four builtins), `executor` (dispatch builtin vs. subprocess), and `shell` (the REPL loop). Data flows as plain values: `&str` → `Vec<String>` tokens → `Command` → `ExecOutcome`.

**Tech Stack:** Rust (edition 2024), `rustyline` (line editing/history/prompt-level Ctrl-C), `signal-hook` (SIGINT handler so the shell survives Ctrl-C while a child runs).

**Spec:** `docs/superpowers/specs/2026-05-14-shuck-shell-design.md`

---

## File Structure

| File | Responsibility |
|------|----------------|
| `src/main.rs` | Module declarations; entry point — runs the shell, exits with its return code. |
| `src/lexer.rs` | `tokenize(&str) -> Result<Vec<String>, LexError>` — a character state machine handling whitespace, single/double quotes, backslash escapes. |
| `src/command.rs` | `Command { program, args }` struct and `parse(Vec<String>) -> Option<Command>`. |
| `src/builtins.rs` | `ExecOutcome` enum, `is_builtin`, `run_builtin`, and the four builtin implementations. |
| `src/executor.rs` | `execute(&Command) -> ExecOutcome` — runs a builtin or spawns a subprocess and waits. |
| `src/shell.rs` | `run() -> i32` — the REPL loop, SIGINT handler installation, line processing. |

**Note on intermediate builds:** Until Task 7 wires everything into `main.rs`, a plain `cargo build` may emit `function is never used` warnings. That is expected. Per-task verification uses `cargo test`, where the test modules exercise the code, so those builds are clean.

---

## Task 1: Project setup — dependencies and module skeleton

**Files:**
- Modify: `Cargo.toml` (via `cargo add`)
- Create: `src/lexer.rs`, `src/command.rs`, `src/builtins.rs`, `src/executor.rs`, `src/shell.rs` (all empty)
- Modify: `src/main.rs`

- [ ] **Step 1: Add dependencies**

Run:
```bash
cargo add rustyline signal-hook
```
Expected: `Cargo.toml` gains `rustyline` and `signal-hook` under `[dependencies]`.

- [ ] **Step 2: Create the five empty module files**

Create `src/lexer.rs`, `src/command.rs`, `src/builtins.rs`, `src/executor.rs`, and `src/shell.rs` as empty files (zero bytes). They are valid empty modules and later tasks fill them in.

- [ ] **Step 3: Wire module declarations into `src/main.rs`**

Replace the entire contents of `src/main.rs` with:

```rust
mod builtins;
mod command;
mod executor;
mod lexer;
mod shell;

fn main() {
    println!("shuck: setup complete");
}
```

- [ ] **Step 4: Verify the project builds**

Run: `cargo build`
Expected: PASS — compiles with no errors. (No warnings yet, since the modules are empty.)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock src/
git commit -m "chore: add dependencies and module skeleton"
```

---

## Task 2: Lexer — basic whitespace tokenization

**Files:**
- Modify: `src/lexer.rs`
- Test: `src/lexer.rs` (inline `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing tests**

Put this in `src/lexer.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple_command() {
        assert_eq!(tokenize("ls -la").unwrap(), vec!["ls", "-la"]);
    }

    #[test]
    fn tokenize_empty_input() {
        assert_eq!(tokenize("").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn tokenize_only_whitespace() {
        assert_eq!(tokenize("   \t  ").unwrap(), Vec::<String>::new());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib lexer`
Expected: FAIL — `cannot find function tokenize in this scope`.

- [ ] **Step 3: Write the minimal implementation**

Add this above the `#[cfg(test)]` module in `src/lexer.rs`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
}

pub fn tokenize(input: &str) -> Result<Vec<String>, LexError> {
    let tokens = input.split_whitespace().map(|s| s.to_string()).collect();
    Ok(tokens)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib lexer`
Expected: PASS — 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat: add basic whitespace tokenizer"
```

---

## Task 3: Lexer — quotes, escapes, and errors

**Files:**
- Modify: `src/lexer.rs`
- Test: `src/lexer.rs` (inline `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing tests**

Add these tests inside the existing `#[cfg(test)] mod tests` block in `src/lexer.rs` (keep the three tests from Task 2):

```rust
    #[test]
    fn tokenize_single_quotes() {
        assert_eq!(
            tokenize("echo 'hello world'").unwrap(),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn tokenize_double_quotes() {
        assert_eq!(
            tokenize("echo \"hello world\"").unwrap(),
            vec!["echo", "hello world"]
        );
    }

    #[test]
    fn tokenize_double_quote_escape() {
        assert_eq!(tokenize(r#"echo "a\"b""#).unwrap(), vec!["echo", "a\"b"]);
    }

    #[test]
    fn tokenize_backslash_escape_outside_quotes() {
        assert_eq!(tokenize(r"echo a\ b").unwrap(), vec!["echo", "a b"]);
    }

    #[test]
    fn tokenize_adjacent_runs_concatenate() {
        assert_eq!(tokenize(r#"foo"bar baz""#).unwrap(), vec!["foo bar baz"]);
    }

    #[test]
    fn tokenize_single_quotes_preserve_backslash() {
        assert_eq!(tokenize(r"echo 'a\b'").unwrap(), vec!["echo", r"a\b"]);
    }

    #[test]
    fn tokenize_empty_quotes_produce_empty_token() {
        assert_eq!(tokenize("''").unwrap(), vec![""]);
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib lexer`
Expected: FAIL — the new quote/escape tests fail (the `split_whitespace` implementation does not handle quotes). The three Task 2 tests still pass.

- [ ] **Step 3: Replace `tokenize` with the full state machine**

Replace the `tokenize` function in `src/lexer.rs` with this (leave the `LexError` enum as it is):

```rust
pub fn tokenize(input: &str) -> Result<Vec<String>, LexError> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if has_token {
                tokens.push(std::mem::take(&mut current));
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
            other => {
                has_token = true;
                current.push(other);
            }
        }
    }

    if has_token {
        tokens.push(current);
    }
    Ok(tokens)
}
```

Notes baked into this implementation:
- `has_token` is set `true` on entering any quote, so `''` yields one empty-string token.
- Single quotes are fully literal (a backslash inside stays a backslash).
- Inside double quotes, `\` only escapes `"` and `\`; any other `\x` keeps the backslash literally.
- A trailing `\` with no following character is treated as a literal backslash.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib lexer`
Expected: PASS — all 12 lexer tests pass (3 from Task 2 + 9 new).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat: handle quotes and escapes in tokenizer"
```

---

## Task 4: Command parsing

**Files:**
- Modify: `src/command.rs`
- Test: `src/command.rs` (inline `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing tests**

Put this in `src/command.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), None);
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec!["ls".to_string()]),
            Some(Command {
                program: "ls".to_string(),
                args: vec![],
            })
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![
                "ls".to_string(),
                "-la".to_string(),
                "/tmp".to_string()
            ]),
            Some(Command {
                program: "ls".to_string(),
                args: vec!["-la".to_string(), "/tmp".to_string()],
            })
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib command`
Expected: FAIL — `cannot find type Command` / `cannot find function parse`.

- [ ] **Step 3: Write the implementation**

Add this above the `#[cfg(test)]` module in `src/command.rs`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

pub fn parse(tokens: Vec<String>) -> Option<Command> {
    let mut iter = tokens.into_iter();
    let program = iter.next()?;
    let args = iter.collect();
    Some(Command { program, args })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib command`
Expected: PASS — 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/command.rs
git commit -m "feat: add command parsing"
```

---

## Task 5: Builtins

**Files:**
- Modify: `src/builtins.rs`
- Test: `src/builtins.rs` (inline `#[cfg(test)]` module)

This task defines the `ExecOutcome` enum (used by `builtins`, `executor`, and `shell`), the four builtins, `is_builtin`, and `run_builtin`. `cd` and `pwd` mutate real process/filesystem state and are smoke-tested manually in Task 8; `is_builtin` and `exit` are pure and get unit tests here.

- [ ] **Step 1: Write the failing tests**

Put this in `src/builtins.rs`:

```rust
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib builtins`
Expected: FAIL — `cannot find function is_builtin` / `builtin_exit` / type `ExecOutcome`.

- [ ] **Step 3: Write the implementation**

Add this above the `#[cfg(test)]` module in `src/builtins.rs`:

```rust
use std::env;
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

/// Runs a builtin. Caller must ensure `is_builtin(name)` is true.
pub fn run_builtin(name: &str, args: &[String]) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args),
        "pwd" => builtin_pwd(),
        "echo" => builtin_echo(args),
        "exit" => builtin_exit(args),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}

fn builtin_cd(args: &[String]) -> ExecOutcome {
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

fn builtin_pwd() -> ExecOutcome {
    match env::current_dir() {
        Ok(path) => {
            println!("{}", path.display());
            ExecOutcome::Continue(0)
        }
        Err(e) => {
            eprintln!("shuck: pwd: {e}");
            ExecOutcome::Continue(1)
        }
    }
}

fn builtin_echo(args: &[String]) -> ExecOutcome {
    println!("{}", args.join(" "));
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib builtins`
Expected: PASS — 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs
git commit -m "feat: add cd, pwd, echo, exit builtins"
```

---

## Task 6: Executor

**Files:**
- Modify: `src/executor.rs`

The executor dispatches a `Command` to either a builtin or a spawned subprocess. It depends on `command::Command` and `builtins` (which owns `ExecOutcome`). It is smoke-tested manually in Task 8 since it spawns real processes.

- [ ] **Step 1: Write the implementation**

Put this in `src/executor.rs`:

```rust
use std::io::ErrorKind;
use std::process::Command as ProcessCommand;

use crate::builtins::{self, ExecOutcome};
use crate::command::Command;

pub fn execute(cmd: &Command) -> ExecOutcome {
    if builtins::is_builtin(&cmd.program) {
        return builtins::run_builtin(&cmd.program, &cmd.args);
    }

    match ProcessCommand::new(&cmd.program).args(&cmd.args).status() {
        Ok(status) => ExecOutcome::Continue(status.code().unwrap_or(1)),
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

The spawned child inherits the shell's stdin/stdout/stderr by default (that is `std::process::Command`'s default when `status()` is used), so interactive programs work.

- [ ] **Step 2: Verify it compiles**

Run: `cargo test --lib`
Expected: PASS — the whole library compiles and all existing tests (lexer, command, builtins) still pass. `executor` has no tests of its own; this step confirms it compiles and integrates.

- [ ] **Step 3: Commit**

```bash
git add src/executor.rs
git commit -m "feat: add executor for builtins and subprocesses"
```

---

## Task 7: Shell REPL and final wiring

**Files:**
- Modify: `src/shell.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write the shell REPL**

Put this in `src/shell.rs`:

```rust
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use signal_hook::consts::SIGINT;

use crate::builtins::ExecOutcome;
use crate::command;
use crate::executor;
use crate::lexer::{self, LexError};

const PROMPT: &str = "shuck> ";

/// Runs the interactive shell loop. Returns the process exit code.
pub fn run() -> i32 {
    install_sigint_handler();

    let mut editor = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("shuck: failed to initialize line editor: {e}");
            return 1;
        }
    };

    loop {
        match editor.readline(PROMPT) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match process_line(&line) {
                    ExecOutcome::Exit(code) => return code,
                    ExecOutcome::Continue(_) => {}
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => return 0,
            Err(e) => {
                eprintln!("shuck: input error: {e}");
                return 1;
            }
        }
    }
}

/// Installs a SIGINT handler so the shell survives Ctrl-C while a child
/// process runs. The handler is a real handler (not SIG_IGN), so a spawned
/// child resets SIGINT to its default disposition on exec and is terminated
/// normally. The flag itself is not read; registering the handler is the
/// whole point.
fn install_sigint_handler() {
    let flag = Arc::new(AtomicBool::new(false));
    if let Err(e) = signal_hook::flag::register(SIGINT, flag) {
        eprintln!("shuck: warning: could not install SIGINT handler: {e}");
    }
}

/// Tokenizes, parses, and executes a single input line.
fn process_line(line: &str) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(LexError::UnterminatedQuote) => {
            eprintln!("shuck: syntax error: unterminated quote");
            return ExecOutcome::Continue(2);
        }
    };

    match command::parse(tokens) {
        Some(cmd) => executor::execute(&cmd),
        None => ExecOutcome::Continue(0),
    }
}
```

- [ ] **Step 2: Wire `main.rs` to run the shell**

Replace the entire contents of `src/main.rs` with:

```rust
mod builtins;
mod command;
mod executor;
mod lexer;
mod shell;

fn main() {
    std::process::exit(shell::run());
}
```

- [ ] **Step 3: Verify the project builds cleanly and tests pass**

Run: `cargo build`
Expected: PASS — compiles with **no warnings** (every module is now reachable from `main`).

Run: `cargo test`
Expected: PASS — all 19 unit tests pass (12 lexer + 3 command + 4 builtins).

- [ ] **Step 4: Commit**

```bash
git add src/shell.rs src/main.rs
git commit -m "feat: add REPL loop and wire up the shell"
```

---

## Task 8: Manual smoke test

**Files:** none (verification only)

The builtins and executor mutate real process/filesystem state, so they are verified by hand here.

- [ ] **Step 1: Build and launch**

Run: `cargo run`
Expected: the `shuck> ` prompt appears.

- [ ] **Step 2: Run through this checklist at the prompt**

| Input | Expected behavior |
|-------|-------------------|
| `pwd` | prints the current working directory |
| `echo hello world` | prints `hello world` |
| `echo "quoted   spaces"` | prints `quoted   spaces` (internal spaces preserved) |
| `echo 'single $quoted'` | prints `single $quoted` literally |
| `cd /tmp` then `pwd` | second line prints `/tmp` |
| `cd` then `pwd` | prints the `$HOME` directory |
| `cd /nonexistent` | prints a `shuck: cd: ...` error, shell continues |
| `ls -la` | runs `/bin/ls`, output appears inline |
| `nonexistent-cmd` | prints `shuck: command not found: nonexistent-cmd` |
| `echo "oops` (unterminated quote) | prints `shuck: syntax error: unterminated quote`, shell continues |
| `sleep 5` then press Ctrl-C | the `sleep` is interrupted; the `shuck> ` prompt returns; shell is still alive |
| press Ctrl-C at an empty prompt | current line is discarded, fresh prompt appears |
| press the Up arrow | the previous command is recalled (history works) |
| `exit 0` | the shell exits cleanly |
| relaunch, then press Ctrl-D at the prompt | the shell exits cleanly |

- [ ] **Step 2: Confirm all rows behave as expected**

If any row misbehaves, stop and fix the relevant module before completing the plan.

---

## Self-Review Notes

- **Spec coverage:** builtins (Task 5), subprocess execution (Task 6), quote/escape lexer (Tasks 2–3), command parsing (Task 4), REPL + Ctrl-C/Ctrl-D + SIGINT (Task 7), resilient error handling (Tasks 5–7), lexer + parse unit tests (Tasks 2–4), manual smoke testing of builtins/executor (Task 8). All spec sections map to a task.
- **Type consistency:** `ExecOutcome` is defined once in `builtins.rs` and consumed by `executor.rs` and `shell.rs`. `Command { program, args }` is defined in `command.rs` and consumed by `executor.rs`. `LexError::UnterminatedQuote` is defined in `lexer.rs` and matched in `shell.rs`. Function signatures (`tokenize`, `parse`, `is_builtin`, `run_builtin`, `execute`, `run`) are consistent across definition and call sites.
- **No placeholders:** every code step contains complete code; every run step has an exact command and expected result.
