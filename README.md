# shuck

A small POSIX-ish shell written in Rust, built incrementally as a learning
project. Each iteration ships a single coherent feature with a written design
spec, an implementation plan, and a test suite.

## Status

| Iteration | Feature                                                 |
| --------- | ------------------------------------------------------- |
| v1        | Core shell: lexer, parser, executor, basic builtins     |
| v2        | Sequencing (`;`, `&&`, `\|\|`)                          |
| v3        | Pipes (`\|`) and redirection (`<`, `>`, `>>`, `2>`)     |
| v4        | Variables and expansion (`$VAR`, `${VAR}`, assignments) |
| v5        | Command substitution (`$(cmd)`)                         |
| v6        | Background jobs (`&`, `jobs`, `wait`)                   |
| v7        | Foreground job control (`fg`, `bg`, Ctrl-Z)             |

## Build and run

```sh
cargo build --release
cargo run                # interactive REPL
cargo test               # full test suite (214 tests)
```

## Features

**Syntax:**
`cmd a b c`, `cmd1 ; cmd2`, `cmd1 && cmd2`, `cmd1 || cmd2`, `cmd1 | cmd2`,
`cmd > out`, `cmd < in`, `cmd >> out`, `cmd 2> err`, `cmd &`,
`echo "$VAR"`, `echo $(date)`, `NAME=value cmd`.

**Builtins:**
`cd`, `pwd`, `echo`, `exit`, `export`, `unset`, `jobs`, `wait`, `fg`, `bg`.

**Job control (v6 + v7):**
Trailing `&` runs a pipeline in its own process group, prints
`[N] PID`, and the prompt-time reaper prints `[N] Done <cmd> &`
notifications. Foreground pipelines also get their own process group;
`tcsetpgrp` hands them the controlling terminal so interactive programs
(`vim`, `less`) work and Ctrl-Z stops the job into `Stopped` state. `fg`
resumes the current job in foreground; `bg` resumes the current stopped
job in background. `jobs` lists Running/Stopped/finished jobs with
`+`/`-` markers; `wait` blocks until no jobs are Running or Stopped, and
can be interrupted with Ctrl-C.

**Not yet implemented:**
job specifiers (`%1`, `%+`, `%-`, `%cmd`), `disown`, `kill` builtin,
control flow (`if`/`while`/`for`/`case`), functions, quoted globbing,
history expansion, arithmetic, here-docs, aliases.

## Project layout

```
src/
  main.rs        entry point
  shell.rs       REPL loop, signal install
  shell_state.rs Shell struct (env, vars, jobs)
  lexer.rs       token stream
  command.rs     parser → AST (Sequence/Pipeline/SimpleCommand)
  expand.rs      variable + command substitution
  executor.rs    fork/exec, pipes, redirects, background spawn
  builtins.rs    builtin dispatch table
  jobs.rs        JobTable + SIGCHLD reaping
docs/superpowers/
  specs/         design spec per iteration
  plans/         implementation plan per iteration
```

## Development workflow

Each iteration follows the same loop:

1. **Brainstorm** → design spec in `docs/superpowers/specs/`
2. **Plan**     → task-by-task plan in `docs/superpowers/plans/`
3. **Implement** task-by-task on a feature branch, with per-task code review
4. **Final review** across the whole branch before merging to `main`

Tests live alongside each module in `#[cfg(test)] mod tests` blocks.

## Dependencies

- `rustyline` — line editing
- `signal-hook` — SIGINT, SIGCHLD
- `libc` — `waitpid`, `setpgid`, `killpg`, `tcsetpgrp`, `signal`

## License

Personal learning project; no license declared.
