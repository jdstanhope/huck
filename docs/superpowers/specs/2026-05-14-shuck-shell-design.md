# shuck — a simple command-line shell

**Date:** 2026-05-14
**Status:** Approved

## Overview

`shuck` is a simple interactive command-line shell written in Rust. This
first version provides a small set of builtin commands and executes external
programs as subprocesses. Pipes and redirection are explicitly out of scope
for this version but the design leaves room for them.

## Goals

- Interactive REPL with line editing and history.
- Builtin commands: `cd`, `exit`, `pwd`, `echo`.
- Run any other command as a subprocess, inheriting stdio, and wait for it.
- Robust: a single bad command (parse error, missing program, builtin
  failure) never crashes the shell — it prints an error and re-prompts.
- Sensible Ctrl-C and Ctrl-D behavior.

## Non-goals (this version)

- Pipes (`|`) and redirection (`>`, `<`, `>>`).
- Environment variable expansion (`$VAR`) and tilde (`~`) expansion.
- Job control / background jobs.
- Scripting (running a file of commands), globbing, command substitution.

## Dependencies

- `rustyline` — line editing, history, and prompt-level Ctrl-C handling.
- `signal-hook` (or `ctrlc`) — installs a SIGINT handler so the shell
  survives Ctrl-C while a child process is running.

## Architecture

```
src/
  main.rs       — entry point; constructs Shell, runs it
  shell.rs      — REPL loop: prompt, read line (rustyline), dispatch, SIGINT setup
  lexer.rs      — tokenize(line) -> Result<Vec<String>, LexError>
  command.rs    — Command { program, args }; parse(tokens) -> Option<Command>
  builtins.rs   — cd, exit, pwd, echo; is_builtin(name); run_builtin(...)
  executor.rs   — execute(Command): dispatches builtin vs subprocess
```

Each module has one responsibility and communicates through plain data
(`Vec<String>` tokens, a `Command` struct). The lexer and command parser are
pure functions and unit-testable in isolation.

### Data flow per input line

```
read line
  -> lexer::tokenize        (line -> Vec<String> tokens, or LexError)
  -> command::parse         (tokens -> Option<Command>)
  -> executor::execute      (builtin dispatch | spawn subprocess + wait)
```

## Components

### lexer.rs

`tokenize(input: &str) -> Result<Vec<String>, LexError>`

A hand-written character state machine:

- Whitespace separates tokens when outside quotes.
- `'single quotes'`: every character literal until the closing `'`; no
  escapes recognized inside.
- `"double quotes"`: characters literal except `\`, which escapes `"` and
  `\`. (Other `\x` sequences inside double quotes are left as-is, i.e. the
  backslash is preserved.)
- `\` outside quotes escapes the next character literally (including spaces),
  so the next character does not act as a separator or quote.
- Adjacent quoted/unquoted runs concatenate into a single token:
  `foo"bar baz"` produces one token `foo bar baz`.
- `LexError::UnterminatedQuote` is returned when input ends inside a quote.

### command.rs

```rust
struct Command {
    program: String,
    args: Vec<String>,
}
```

`parse(tokens: Vec<String>) -> Option<Command>`

- Empty token list -> `None` (the shell simply re-prompts).
- Otherwise `program = tokens[0]`, `args = tokens[1..]`.

### builtins.rs

- `is_builtin(name: &str) -> bool`
- `run_builtin(name, args, shell) -> ...` returning an exit status / control
  signal (e.g. whether the shell should exit).

Builtins:

- `cd`: `cd <dir>`; with no argument, change to `$HOME`. Uses
  `std::env::set_current_dir`. On failure, prints an error and continues.
- `pwd`: prints `std::env::current_dir()`.
- `echo`: prints its arguments joined by single spaces, followed by a newline.
- `exit`: `exit [code]`; defaults to `0`. A non-numeric code prints an error
  and does *not* exit.

`cd` and `exit` must be builtins because they mutate or end the shell process
itself; a subprocess could not do this.

### executor.rs

`execute(cmd: Command, shell: &mut Shell)`

- If `is_builtin(&cmd.program)` -> `run_builtin`.
- Otherwise spawn via `std::process::Command`, inheriting stdin/stdout/stderr,
  and wait for it to finish.
- Program not found -> print `shuck: command not found: <name>` and continue.
- Any spawn/wait error prints to stderr and returns control to the prompt.
- The exit status of the last command is stored on the `Shell` struct. It is
  not surfaced in the prompt yet, but this leaves room for `$?` later.

### shell.rs

Owns the REPL:

- Holds shell state: the `rustyline` editor, last exit status.
- Prints the prompt `shuck> `.
- Reads a line via `rustyline`; non-empty lines are added to history.
- Runs `tokenize -> parse -> execute`. A `LexError` prints an error message
  and re-prompts.
- Installs the SIGINT handler (via `signal-hook`/`ctrlc`) at startup.

## Error handling

The shell is resilient by construction: every per-line failure mode
(lex error, empty command, command not found, spawn failure, builtin error)
prints a message and returns to the prompt. Nothing short of `exit` or
Ctrl-D (EOF) terminates the shell.

## Signal behavior

- **Ctrl-C at the prompt:** `rustyline` returns `Interrupted`; the shell
  discards the current line and prints a fresh prompt.
- **Ctrl-C while a child runs:** the shell installs a SIGINT *handler*
  (not `SIG_IGN`). On `exec`, a child resets handled signals to their default
  disposition, so the child is terminated normally by Ctrl-C while the
  shell's handler keeps the shell alive.
- **Ctrl-D (EOF) at the prompt:** the shell exits cleanly.

## Testing

- **Unit tests** on `lexer::tokenize`: whitespace splitting, single quotes,
  double quotes with escapes, backslash escapes outside quotes, adjacent-run
  concatenation, and the unterminated-quote error.
- **Unit tests** on `command::parse`: empty input, program-only, program with
  arguments.
- **Builtins and executor:** manual smoke testing for this version, since
  they mutate real process and filesystem state.

## Future extensions (not in scope)

- Pipes: extend the lexer to recognize `|`, parse into a pipeline of
  `Command`s, and have the executor wire up stdio between them.
- Redirection: recognize `>`, `<`, `>>` and configure child stdio.
- `$VAR` / `~` expansion as a stage between parsing and execution.
