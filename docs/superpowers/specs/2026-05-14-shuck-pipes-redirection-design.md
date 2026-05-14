# shuck — pipes and redirection

**Date:** 2026-05-14
**Status:** Approved
**Builds on:** `2026-05-14-shuck-shell-design.md` (shuck shell v1)

## Overview

This adds pipelines and I/O redirection to `shuck`. v1 runs exactly one
command per line; this version lets a line be a pipeline of commands
connected by `|`, with each command optionally redirecting its standard
streams to or from files.

## Goals

- Support these operators:
  - `|`  — connect stdout of one command to stdin of the next
  - `>`  — redirect stdout to a file (truncate)
  - `>>` — redirect stdout to a file (append)
  - `<`  — redirect a file into stdin
  - `2>` — redirect stderr to a file (truncate)
  - `2>>`— redirect stderr to a file (append)
- Operators do not require surrounding whitespace (`echo hi>f` works).
- Quoted or escaped operator characters are literal text, not operators.
- Builtins participate: `echo`/`pwd` output can be redirected or piped.
- A pipeline's exit status is the exit status of its last command.

## Non-goals (this version)

- `&&`, `||`, `;` command sequencing.
- Here-documents (`<<`), here-strings (`<<<`).
- Duplicating file descriptors (`2>&1`, `>&`).
- Redirecting arbitrary fds (`3>`, `4<`).
- Background jobs (`&`), job control.
- `$VAR` / `~` / glob expansion (still out of scope, as in v1).

## Architecture

The v1 data flow was `&str -> Vec<String> -> Command -> ExecOutcome`. This
version becomes:

```
&str
  -> lexer::tokenize       -> Vec<Token>             (Word | Op)
  -> command::parse        -> Option<Pipeline>       (or ParseError)
  -> executor::execute     -> ExecOutcome
```

The module boundaries are unchanged — `lexer`, `command`, `executor`,
`builtins`, `shell` — but the types passed between them grow richer.

## Components

### lexer.rs

`tokenize` changes its return type from `Result<Vec<String>, LexError>` to
`Result<Vec<Token>, LexError>`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Op(Operator),
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
```

State-machine additions, all applying only **outside quotes and not
escaped**:

- `|` → emit `Op(Pipe)`, ending any current word.
- `<` → emit `Op(RedirIn)`, ending any current word.
- `>` → look ahead one char: another `>` consumes it and emits
  `Op(RedirAppend)`, otherwise `Op(RedirOut)`.
- `2>` / `2>>` — a `2` is the start of a stderr operator **only when it
  would otherwise begin a new word** (i.e. there is no current word being
  built). In that case, a following `>` (and optional second `>`) emits
  `Op(RedirErr)` or `Op(RedirErrAppend)`. A `2` that is part of or appended
  to an existing word (e.g. `x2>f`) is ordinary word text and the `>` after
  it is a plain `RedirOut`. This matches `bash`.
- Quoted (`"|"`, `'>'`) or escaped (`\|`) operator characters are appended
  to the current word as literal text — they never become `Op` tokens.

`LexError` is unchanged (`UnterminatedQuote`).

### command.rs

The single `Command` struct is replaced by a richer command plus a
`Pipeline` wrapper:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum Redirect {
    Truncate(String), // > file   (and the target form of 2>)
    Append(String),   // >> file  (and the target form of 2>>)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,    // < file
    pub stdout: Option<Redirect>, // > / >> file
    pub stderr: Option<Redirect>, // 2> / 2>> file
}

#[derive(Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>, // invariant: never empty
}
```

`parse` changes from `parse(Vec<String>) -> Option<Command>` to:

```rust
pub fn parse(tokens: Vec<Token>) -> Result<Option<Pipeline>, ParseError>;

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,        // empty pipeline segment: leading/trailing/double |
    MissingRedirectTarget, // redirect operator with no following filename
    RedirectTargetIsOperator, // redirect operator followed by another operator
}
```

- Empty token list → `Ok(None)` (REPL just re-prompts).
- Otherwise the parser walks tokens left to right, building one `Command`
  at a time:
  - A `Word`: the first becomes `program`, subsequent ones are `args`.
  - `Op(Pipe)`: finish the current command and start a new one. If the
    current command has no `program`, return `Err(MissingCommand)`.
  - A redirect `Op`: the **next** token must be a `Word`, used as the
    filename. Missing next token → `Err(MissingRedirectTarget)`; next token
    is an `Op` → `Err(RedirectTargetIsOperator)`. The matching field
    (`stdin` / `stdout` / `stderr`) is set; if already set, it is
    overwritten (**last redirect of a kind wins**).
  - End of input: finish the last command; if it has no `program`, return
    `Err(MissingCommand)`.
- A returned `Pipeline` always has at least one `Command`.

### builtins.rs

`echo` and `pwd` are refactored to write their output to a caller-supplied
`&mut dyn Write` instead of using `println!`, so the executor can direct
that output to the terminal, a file, or a pipe buffer. `cd` and `exit`
produce no stdout and keep their current signatures. `run_builtin` gains a
writer parameter that it forwards to `echo`/`pwd` and ignores for
`cd`/`exit`. `ExecOutcome`, `is_builtin`, and the four builtins' core logic
are otherwise unchanged. Builtin error messages continue to go to the
process's real stderr via `eprintln!` (stderr redirection of builtins is
not in scope).

### executor.rs

`execute` changes from `execute(&Command) -> ExecOutcome` to
`execute(&Pipeline) -> ExecOutcome`.

**Opening redirect files.** Before running anything, the executor opens
every redirect file the pipeline needs. `<` opens read-only; `Truncate`
opens write + create + truncate; `Append` opens write + create + append.
If any open fails, the executor prints `shuck: <file>: <error>` to stderr,
runs nothing, and returns `ExecOutcome::Continue(1)`.

**Single-command pipeline.**
- Subprocess: spawn with `std::process::Command`, setting `stdin`/`stdout`/
  `stderr` to `Stdio::from(file)` for any redirected stream; unredirected
  streams inherit the shell's. Wait, return `Continue(code)` (the v1
  `128 + signal` rule for signal-killed children still applies).
- Builtin: build the stdout writer — the opened `>`/`>>` file if present,
  otherwise the real stdout — and call `run_builtin` with it. `cd`/`exit`
  behave exactly as in v1 (including `cd` changing the shell's directory
  and `exit` returning `ExecOutcome::Exit`).

**Multi-command pipeline.** Commands are spawned left to right. For command
*i*, its stdin is the read end of the pipe from command *i-1* (or, for the
first command, inherited / the `<` file), and its stdout is a new pipe to
command *i+1* (or, for the last command, inherited / the `>` file). An
explicit redirect on a command overrides the corresponding pipe end for
that command.

- **Subprocess stages** use `Stdio::piped()` and the standard
  `child.stdout.take()` → `Stdio::from(...)` chaining.
- **Builtin stages** do not spawn a process. `echo`/`pwd` write to an
  in-memory `Vec<u8>`; that buffer is then written to the next stage's
  stdin (or to the final stdout/redirect if the builtin is the last
  stage). Builtins do not read stdin, so an incoming pipe to a builtin
  stage is simply closed/ignored. `cd` and `exit` appearing in a
  multi-command pipeline are **no-ops that return `Continue(0)`** — their
  shell-state effects (directory change, termination) are dropped, matching
  the "pipeline stage runs in a subshell" intuition without forking the
  shell.

After spawning, the executor waits on all subprocess stages. The
**pipeline's exit status is the last command's exit status**; that is the
`ExecOutcome` returned (unless the last stage is a standalone `exit`
builtin, which still returns `ExecOutcome::Exit`).

### shell.rs

Only `process_line` changes. `command::parse` now returns
`Result<Option<Pipeline>, ParseError>`:

- `Err(e)` → print `shuck: syntax error: <description of e>` and return
  `ExecOutcome::Continue(2)` (mirrors the existing `LexError` arm).
- `Ok(None)` → return `ExecOutcome::Continue(0)`.
- `Ok(Some(pipeline))` → `executor::execute(&pipeline)`.

The REPL loop, history, SIGINT handling, and EOF behavior are unchanged.

## Error handling

| Situation | Behavior |
|-----------|----------|
| Unterminated quote | `shuck: syntax error: unterminated quote`, `Continue(2)` |
| Empty pipeline segment (`\| cmd`, `cmd \|`, `cmd \|\| cmd`) | `shuck: syntax error: ...`, `Continue(2)` |
| Redirect operator with no filename | `shuck: syntax error: ...`, `Continue(2)` |
| Redirect file cannot be opened | `shuck: <file>: <os error>`, `Continue(1)`, nothing runs |
| Command not found in a pipeline stage | that stage prints `shuck: command not found: <name>`; other stages still run; that stage contributes status 127 |
| Signal-killed child | `128 + signal` status (v1 rule, unchanged) |

A single bad line never crashes the shell — every failure path returns an
`ExecOutcome` and the REPL re-prompts.

## Testing

- **Lexer unit tests:** each operator emitted correctly; `>>` / `2>` /
  `2>>` lookahead; operators with no surrounding whitespace
  (`echo hi>f`); quoted and escaped operator characters stay `Word`
  tokens; a `2` mid-word does not trigger the stderr operator.
- **Parser unit tests:** single command with each redirect kind;
  multi-stage pipeline; last-redirect-of-a-kind wins; every `ParseError`
  variant (missing command at leading/trailing/double pipe, missing
  redirect target, redirect target is an operator); empty input → `None`.
- **Executor:** manual smoke testing with real processes, files, and
  pipes — consistent with how v1 verified the executor. Smoke checklist
  covers: `|` between subprocesses, `>` / `>>` / `<`, `2>` / `2>>`,
  `echo ... > file`, `echo ... | cmd`, a builtin mid-pipeline, redirect
  open failure, and a not-found command inside a pipeline.

## Future extensions (still not in scope)

- `2>&1` and fd duplication.
- `&&` / `||` / `;` sequencing.
- Here-documents and here-strings.
- Background jobs and job control.
