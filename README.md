# huck

A bash-compatible shell written in Rust, working toward being a drop-in
replacement for everyday interactive and scripting use. huck implements most of
bash's surface — expansions, control flow, functions, arrays, job control, line
editing, programmable completion — and verifies it against real bash: every
feature ships with a design spec, an implementation plan, a test suite, and a
**byte-identical bash-diff harness** that runs the same fragments through both
shells and asserts identical output.

Real-world bar: huck sources a non-trivial `~/.bashrc` (bash-completion, git
prompt, nvm, mise activation) and drives interactive tab completion against the
system `bash-completion` package.

## Status

Actively developed, one coherent feature at a time. Current scope:

- **~2,900 tests** (unit + integration) and **45 bash-diff harnesses**, all
  green; `cargo clippy --all-targets` clean.
- Sources `~/.bashrc`-class startup files and the system `bash-completion`
  framework without errors.
- Known gaps and deliberate divergences are tracked exhaustively in
  [`docs/bash-divergences.md`](docs/bash-divergences.md) (Tiers: bugs, missing
  features, intentional, low-impact) — see **Known differences from bash** below
  for the summary.

The full feature history (every iteration's spec and plan) lives in
`docs/superpowers/`.

## Build and run

```sh
cargo build --release
cargo run                # interactive REPL
cargo test               # full test suite
```

## What huck supports

**Command syntax & operators**
Simple commands and pipelines (`a | b`); lists with `;`, `&&`, `||`, and `&`
(background, including backgrounding an and-or group and `&` as a list
separator); grouping with `( … )` (subshell) and `{ …; }` (current shell);
redirections `<`, `>`, `>>`, `2>`, `2>>`, `&>`, `&>>`, fd duplication (`2>&1`,
`1>&2`), here-documents (`<<`, `<<-`), and here-strings (`<<<`); redirections on
compound commands (`while …; done > file`). Comments (`#`), line continuation
(`\`), and multi-line input (an open quote/expansion/compound/operator carries
onto a `>` continuation prompt).

**Expansions**
- Parameter: `$VAR`, `${VAR}`, positional (`$1`, `${10}`, `$@`, `$*`, `$#`),
  specials (`$?`, `$$`, `$!`, `$0`, `$-`, `$PIPESTATUS`).
- Modifiers: `${v:-w}` / `:=` / `:?` / `:+` (and non-`:` forms), length `${#v}`,
  prefix/suffix strip `${v#p}` / `##` / `%` / `%%`, substring `${v:off:len}`,
  pattern substitution `${v/p/r}` / `//` / `/#` / `/%`, case modification
  `${v^^}` / `,,` / `^` / `,`, transforms `${v@Q}` / `@P` / `@U` / `@L` / `@u`
  / `@E`, indirection `${!v}`, prefix-name and array-key `${!a[@]}`.
- Arithmetic: `$((…))`, `((…))`, and C-style `for ((;;))` — full operator set
  including bitwise, assignment, `++`/`--`, `**`, ternary, comma, non-decimal
  bases.
- Command substitution `$(…)` and `` `…` ``.
- Brace expansion `{a,b}` / `{1..9}` / `{a..z}` (nested, stepped, zero-padded);
  tilde `~`, `~user`, `~+`, `~-`; pathname globbing `*`, `?`, `[…]`, `[!…]`,
  `[^…]`, POSIX classes `[[:alpha:]]`, and `extglob` (`?(…)`/`*(…)`/`+(…)`/
  `@(…)`/`!(…)`) for both matching and pathname generation; word-splitting on
  `$IFS`.

**Control flow & functions**
`if`/`elif`/`else`, `while`/`until`, `for` (word-list, `"$@"`, and C-style),
`select`, `case` (with `;;` / `;&` / `;;&`); `break N` / `continue N`. Functions
in both `name() { … }` and `function name { … }` forms, with positional args,
`local` (and `declare`/`typeset` attributes), `return`, and dynamic scoping.
`[[ … ]]` extended test: glob `==`/`!=`, regex `=~` (populating `BASH_REMATCH`),
`-v`, `-o optname`, file/string/integer tests, `&&`/`||`/`!`/grouping.

**Variables & arrays**
Scalars, indexed arrays (`a=(x y)`, `a[i]=`, `a+=`, `${a[@]}`, `${!a[@]}`,
slicing), associative arrays (`declare -A`), integer (`-i`), readonly (`-r`),
export (`-x`) attributes; `declare -g`; `printf -v`.

**Job control**
Foreground and background process groups, `tcsetpgrp` terminal handoff (so
`vim`/`less` and Ctrl-Z work), SIGCHLD reaping with `[N] Done` notices, and
`jobs`/`fg`/`bg`/`wait`/`kill`/`disown` with `%N`/`%+`/`%%`/`%-`/`%cmd`/bare-PID
specifiers.

**Line editing, history & completion**
A line editor with history (persisted to `$HISTFILE`), history expansion
(`!!`, `!n`, `!str`, `!$`, `^old^new^`, …), and programmable tab completion:
command/file/variable completion plus the full `complete`/`compgen`/`compopt`
machinery (`-F` functions, `-W` wordlists, action sets, `-D` default), which
drives the system `bash-completion` framework.

**Builtins & options**
`cd`, `pwd`, `echo`, `printf` (incl. `%q`), `read`, `test`/`[`, `[[`, `export`,
`readonly`, `local`, `declare`/`typeset`, `unset`, `set` (`-e`/`-u`/`-x`/`-f`/
`-o`/`-o pipefail`/`set --`/`shift`), `shopt`, `getopts`, `eval`, `command`,
`hash`, `trap` (EXIT/ERR/DEBUG/RETURN + signals), `alias`/`unalias`, `jobs`,
`fg`, `bg`, `wait`, `kill`, `disown`, `history`, `break`, `continue`, `return`,
`exit`, `complete`/`compgen`/`compopt`.

## Known differences from bash

huck targets byte-identical behavior; remaining differences are tracked in
[`docs/bash-divergences.md`](docs/bash-divergences.md) (the authoritative,
exhaustive list, tiered by severity). In summary:

**Not yet implemented** (parity backlog — bash accepts these, huck doesn't yet):
- Redirections: `>|` (noclobber override), `n<>file` (read-write open), `|&`
  (pipe stdout+stderr shorthand); process substitution `<(…)` / `>(…)`.
- Array I/O: `mapfile`/`readarray`, `read -a`/`-A`; array/attribute transforms
  `${v@A}` / `@K` / `@k` / `@a`; prefix-name `${!prefix@}` / `${!prefix*}`.
- Some `set`/`declare` modes: `set -n` (noexec), `-C` (noclobber), `-b`, `-h`,
  job-control `monitor`; `declare -l`/`-u`/`-n` (lowercase/uppercase/nameref);
  integer/exported arrays.
- Misc: `$"…"` locale quoting, `cd -P`/`-L`, `pwd -P`/`-L`, `FUNCNAME`,
  `history -d`/`-w`/`-r`/`-a` and `history N`, `HISTSIZE`/`HISTFILESIZE`, the
  full signal-name table and `kill -<negative-PID>`, and `test -v` (the `[[ ]]`
  form works).

**Intentional divergences** (kept on purpose):
- `[[ =~ ]]` uses Rust's `regex` engine (RE2-style), not POSIX ERE — no
  backreferences, and leftmost-longest/alternation semantics can differ.
- Associative-array iteration order is insertion order, not bash 5.x's
  hash-table order.
- Arithmetic shift counts outside `[0, 64)` are an explicit error (bash leaves
  them C-undefined); `[[ < ]]`/`>` compare by byte value (no `LC_COLLATE`).
- `$'\xHH'`/`\nnn` above `0x7F` decode to a Unicode code point, not a raw byte.
- `set -x` uses a flat `$PS4` prefix (no per-depth nesting / PS4 escapes).

**Low-impact / cosmetic**
- Diagnostics use a `huck:` prefix rather than bash's `script: line N:` form.
- History expansion runs on piped non-interactive stdin (bash only does it
  interactively); `huck script.sh` / `source` match bash.
- A handful of pathological edge cases (e.g. `&` inside `$( ( … ) )`) are
  documented in the divergences doc.

Note: a *fully* working `mise<TAB>` additionally requires `bash-completion`
**2.12+** (mise's generated completion calls the 2.12 API); on systems with
2.11 it falls back the same way it does under bash.

## Project layout

```
src/
  main.rs            entry point
  shell.rs           REPL loop, signal install, source reader
  shell_state.rs     Shell struct (env, vars, jobs, options)
  lexer.rs           tokenizer (with offsets, extglob, regex-operand state)
  command.rs         parser → AST (Sequence/Pipeline/SimpleCommand/compound)
  expand.rs          word/parameter/command/brace/tilde/pathname expansion
  param_expansion.rs ${…} modifiers, transforms, substitution
  arith.rs           $(( )) Pratt parser + evaluator
  executor.rs        fork/exec, pipes, redirects, job control, function calls
  builtins.rs        builtin dispatch (incl. printf, set, declare, read, getopts)
  glob_match.rs      extglob + POSIX-class matcher (string + pathname)
  completion.rs      tab-completion driver
  completion_spec.rs complete/compgen spec model + function invocation
  jobs.rs            JobTable + SIGCHLD reaping
  traps.rs           trap/signal dispatch
docs/
  bash-divergences.md   exhaustive, tiered list of differences from bash
  superpowers/specs/    design spec per iteration
  superpowers/plans/    implementation plan per iteration
```

## Development workflow

Each iteration follows the same loop:

1. **Brainstorm** → design spec in `docs/superpowers/specs/`
2. **Plan**      → task-by-task plan in `docs/superpowers/plans/`
3. **Implement** task-by-task on a feature branch, with per-task spec-compliance
   and code-quality review
4. **Final review** across the whole branch, then merge to `main`
5. **Verify** against bash via a per-feature `tests/scripts/*_diff_check.sh`
   harness, and flip the relevant `docs/bash-divergences.md` entry

Tests live alongside each module in `#[cfg(test)] mod tests` blocks, plus
binary-driven integration tests under `tests/`. Interactive features (tab
completion, history recall, Ctrl-C, job control) are covered by PTY-driven
suites (`tests/pty_interactive.rs`, etc.) using the `expectrl` crate; they skip
gracefully where no PTY is available.

## Dependencies

- `rustyline` — line editing
- `regex` — `[[ =~ ]]` matching
- `glob` — pathname expansion (plain globs)
- `signal-hook` — SIGINT, SIGCHLD
- `libc` — `waitpid`, `setpgid`, `killpg`, `kill`, `tcsetpgrp`, `signal`
- `expectrl` — PTY-driven interactive tests (dev-dependency)

## License

Personal project; no license declared.
