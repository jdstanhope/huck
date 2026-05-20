# huck

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
| v8        | Job specifiers, `kill`, `disown`                        |
| v9        | Tilde expansion (`~`, `~/path`, `~user`, `~+`, `~-`)    |
| v10       | Pathname expansion (`*`, `?`, `[abc]`)                  |
| v11       | Arithmetic expansion (`$((expr))`)                      |
| v12       | Parameter-expansion modifiers (`${var:-w}`, `${var#pat}`, etc.) |
| v13       | Command history + history expansion (`!!`, `!$`, `^a^b^`) |

## Build and run

```sh
cargo build --release
cargo run                # interactive REPL
cargo test               # full test suite (528 tests)
```

## Features

**Syntax:**
`cmd a b c`, `cmd1 ; cmd2`, `cmd1 && cmd2`, `cmd1 || cmd2`, `cmd1 | cmd2`,
`cmd > out`, `cmd < in`, `cmd >> out`, `cmd 2> err`, `cmd &`,
`echo "$VAR"`, `echo $(date)`, `NAME=value cmd`, `cd ~`, `ls ~/dir`,
`cd ~-`, `PATH=~/bin:~/lib`, `ls *.txt`, `echo [ab].rs`, `echo $((2+3))`, `echo ${X:-default}`, `echo ${f##*/}`.

**Builtins:**
`cd`, `pwd`, `echo`, `exit`, `export`, `unset`, `jobs`, `wait`, `fg`, `bg`,
`kill`, `disown`, `history`.

**Job control (v6 + v7 + v8):**
Trailing `&` runs a pipeline in its own process group, prints `[N] PID`,
and the prompt-time reaper prints `[N] Done <cmd> &` notifications.
Foreground pipelines also get their own process group; `tcsetpgrp` hands
them the controlling terminal so interactive programs (`vim`, `less`)
work and Ctrl-Z stops the job into `Stopped` state. `fg`/`bg`/`wait`
accept job specifiers (`%1`, `%+`, `%%`, `%-`); `wait` also accepts a
bare PID and returns the waited-on job's decoded exit status. `kill`
sends signals to PIDs or to a job's process group (`-<sig>` accepts a
name or number, including `-0` for a check-alive probe). `disown`
removes a job from the table without signaling it. `jobs` lists
Running/Stopped/finished jobs with `+`/`-` markers.

**Tilde expansion (v9):**
`~` → `$HOME`, `~/path` → `$HOME/path`, `~+` → `$PWD`, `~-` → `$OLDPWD`,
`~user` → user's home (via `getpwnam_r`). Also expands after unquoted `:`
and `=` in assignment-context words like `PATH=~/bin:~/lib`. Unresolved
forms (missing `HOME`/`PWD`/`OLDPWD`, unknown user) fall back to literal
text. `cd` maintains `PWD` and `OLDPWD`.

**Pathname expansion (v10):**
`*` matches any run of characters, `?` matches one character, `[abc]`
and `[a-z]` match a single character from a class (`[!abc]` negates).
Metacharacters do not cross `/` and do not match a leading `.` (use
`.*` for dotfiles). Quoted metacharacters (`"*"`, `'*'`) stay literal.
A pattern with no matches is passed through unchanged (bash default).
Redirect targets do not yet glob-expand.

**Arithmetic expansion (v11):**
`$((expr))` evaluates a C-style integer expression and substitutes
the decimal result into the surrounding word. Operators: `+`, `-`,
`*`, `/`, `%`, comparison (`==`, `!=`, `<`, `<=`, `>`, `>=`),
logical (`&&`, `||`, `!`) with short-circuit, ternary (`?:`),
parentheses, unary `+`/`-`/`!`. Integers are 64-bit signed and
wrap on overflow (matches bash). Variables are referenced by bare
name (`x`) or with `$` (`$x`); unset/empty values are treated as 0;
non-integer values produce a stderr error and an empty result.
Bitwise operators, assignment operators, increment/decrement, and
non-decimal bases are not implemented.

**Parameter-expansion modifiers (v12):**
Default-value family: `${var:-w}` (use `w` if null), `${var:=w}`
(also assign), `${var:?w}` (stderr error if null), `${var:+w}` (use
`w` if set). The non-`:` variants (`-`/`=`/`?`/`+`) treat only unset
as null. Length: `${#var}` returns the Unicode character count.
Prefix/suffix removal: `${var#pat}`/`${var##pat}` strip the shortest
or longest matching prefix; `${var%pat}`/`${var%%pat}` strip the
suffix. Patterns use glob syntax (`*`, `?`, `[abc]`) and `*` can
cross `/`. The operand `w` (or `pat`) is recursively expanded —
variables, arithmetic, command sub, and tilde all work inside.
Pattern substitution `${var/pat/repl}`, substring `${var:off:len}`,
and case modification are not yet implemented.

**Command history (v13):**
Commands are recorded in memory and persisted to `$HISTFILE` (default
`~/.huck_history`), loaded at startup and saved on exit, capped at
1000 entries. The `history` builtin lists numbered entries; `history
-c` clears them. History expansion runs on each input line before
parsing: `!!` (previous command), `!n` (entry n), `!-n` (n entries
back), `!string` (most recent starting with `string`), `!$` (last
argument), `!^` (first argument), `!*` (all arguments), and
`^old^new^` quick substitution. A `!` is literal inside single
quotes, before whitespace/`=`, or when escaped (`\!`); it still
expands inside double quotes (matching bash). An expanded line is
echoed before it runs. Word designators (`!!:2`) and modifiers
(`:h`/`:t`/`:s`) are not yet implemented.

**Not yet implemented:**
pattern-substitution and substring parameter expansion (`${var/pat/repl}`, `${var:off:len}`),
brace expansion (`{a,b,c}`), special parameters (`$0`/`$1`/`$#`/`$@`/`$$`/`$!`), extended job specs
(`%cmd`/`%?cmd`), `wait -n`, `kill -l`/`-s`, `disown -a`/`-r`/`-h`,
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`), control flow
(`if`/`while`/`for`/`case`), functions, here-docs,
aliases.

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
  job_spec.rs    parser for %N / %+ / %% / %- job specifiers
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
- `libc` — `waitpid`, `setpgid`, `killpg`, `kill`, `tcsetpgrp`, `signal`

## License

Personal learning project; no license declared.
