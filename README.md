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
| v14       | Tab completion (commands, filenames, variables)         |
| v15       | PTY-based interactive test harness                      |
| v16       | `test` / `[` builtin (file, string, integer tests)      |
| v17       | `if` control flow (`if`/`elif`/`else`/`fi`)             |
| v18       | `while`/`until` loops (`break`, `continue`)             |
| v19       | Multi-line input (continuation lines, `> ` prompt)      |
| v20       | `for` loops (`for NAME in WORDS; do … done`)            |
| v21       | `case` statements (`case W in PAT) … ;; esac`)          |
| v22       | Functions (`name() { … }`) + positional parameters      |
| v23       | Inline assignments (`VAR=val cmd`)                       |
| v24       | Here-documents (`<<EOF`, `<<'EOF'`, `<<-EOF`)            |
| v25       | Pipelines as subshells (functions, compounds, builtins    |
|           | all run in forked subshells per POSIX)                    |
| v26       | Special parameters (`$0`, `$$`, `$!`)                    |
| v27       | Here-strings (`<<<word`)                                  |
| v28       | Subshell syntax (`(list)`)                               |
| v29       | FD-duplication redirects (`2>&1`, `1>&2`, `&>file`, `&>>file`) |
| v30       | `[[ ]]` extended test (pattern/regex/int/file/combinators)      |
| v32       | Pattern substitution `${var/pat/repl}` (all six bash forms)      |
| v33       | Substring expansion `${var:off:len}` (M-16)                      |
| v34       | Fatal PE errors (M-58) + `${#1}`/`${#@}`/`${#*}` length (M-60)   |
| v35       | `trap` builtin (M-22 partial — EXIT + 13 real signals)            |
| v36       | `trap` pseudo-signals ERR/DEBUG/RETURN (closes M-22)            |
| v37       | Case modification `${var^^}` / `${var,,}` (M-17)               |
| v38       | Arithmetic completion (M-55 + M-56 + M-57 + `**`)              |
| v39       | ANSI-C quoting `$'…'` (M-28)                                   |
| v40       | `wait -n` + multi-arg `wait` (M-37 + M-38)                     |
| v41       | `kill -l` (M-39) + README cleanup                              |
| v42       | `kill -s SIGNAME` + `kill -n SIGNUM` (M-40)                    |
| v43       | `disown -a`/`-r`/`-h` + SIGHUP-on-exit (M-43)                  |
| v44       | `disown` accepts bare PID (M-44)                               |
| v45       | `jobs -l`/`-p`/`-n`/`-r`/`-s` (M-45)                           |
| v46       | Brace expansion `{a,b,c}` / `{1..5}` (M-61)                    |
| v47       | Extended job specs `%cmd`/`%?cmd` (M-62)                       |
| v48       | Aliases (M-63)                                                 |
| v49       | Backgrounded multi-pipeline sequences (M-52)                   |
| v50       | `shift` + `set --` (M-65)                                      |
| v51       | `source` / `.` (M-66)                                          |
| v52       | `local` (M-67)                                                 |
| v53       | `:` (M-68), `true` / `false` (M-69), `command -v`/`-V` (M-70)  |
| v54       | `readonly` (M-71)                                              |
| v55       | `read` (M-72) + builtin stdin redirections (L-12)              |
| v56       | `printf` (M-73)                                                |
| v57       | `exit` inherits `$?` (M-74)                                    |
| v58       | `type` (M-75)                                                  |
| v59       | `hash` (M-34 partial)                                          |
| v60       | `PS1`/`PS2` prompt customization (M-76)                        |
| v61       | `PROMPT_COMMAND` (M-76 cont.)                                  |
| v62       | rc file: `~/.huckrc` + `--rcfile`/`--norc` (M-77)              |
| v63       | `pushd`/`popd`/`dirs` (M-78)                                   |
| v64       | `declare` / `typeset` (M-79 partial)                           |
| v65       | `declare -i` integer attribute (M-79 cont.)                    |
| v66       | `eval` (M-80)                                                  |
| v67       | `help` (M-81)                                                  |
| v68       | doc cleanup (M-06/M-07/M-35 marked fixed; M-08 narrowed)       |
| v69       | `set -e`/`-u`/`-o` long-form + `$-` (M-08 cont.)               |
| v70       | `cd -` (M-31)                                                  |
| v71       | indexed arrays (M-82)                                          |
| v72       | associative arrays (M-83)                                      |
| v73       | fix `${a[i]:-W}` on missing element (M-82 follow-up)           |
| v74       | configurable IFS (M-05)                                        |
| v75       | test combinators (M-25)                                        |
| v76       | programmable completion: `complete` / `compgen` / `compopt` (M-36 partial) |
| v77       | `function NAME { ... }` keyword form (M-09)                    |
| v78       | C-style `for ((init;cond;step))` + standalone `((expr))` (M-23) |
| v79       | `break N` / `continue N` loop levels (M-30)                     |
| v80       | fix flaky pty test (post-Ctrl-C/Ctrl-Z input race under load)   |
| v81       | `select` loops (M-24) + no-`in` `for` positionals (M-24a)       |
| v82       | script-file mode (`huck script [args]`) + `-c` + `--` (M-77a)   |
| v83       | `set -o pipefail` + `$PIPESTATUS` (M-50)                        |
| v84       | `${var:+(…)}` operands parse as words (metachars literal)        |
| v85       | `!` pipeline negation (`if ! cmd`, `! a \| b`) (M-08c)          |
| v86       | `shopt` builtin: 57-name table + `set -o` bridge + `nullglob`/`dotglob`/`nocaseglob`/`failglob`/`nocasematch` (M-08d) |
| v87       | multi-line `[[ … ]]` + `-v`/`-nt`/`-ot`/`-ef` in `[[ ]]` and `test`/`[` (M-14a) |
| v88       | `complete`/`compgen` actions: full 24-name `-A` set + 12 short flags (`-u`/`-j`/`-v`/…); generates `setopt`/`shopt`/`signal`/`export`/`arrayvar`/etc. (M-36a) |
| v89       | `set -v` verbose mode: echoes each input line to stderr as read (before execution) at both input readers; `v` in `$-`; closes the last `set -v`/`+v` bashrc errors (M-08e) |
| v90       | extglob string matching: `?()`/`*()`/`+()`/`@()`/`!()` (alternation + nesting) in `[[`/`case`/`${}` under `shopt -s extglob`; new backtracking matcher; pathname globbing deferred (M-84, M-84a) |
| v91       | extglob pathname globbing (M-84a): `+(a\|b)` etc. now filesystem-expand via a custom recursive directory walker (reuses the v90 matcher per component; dotfile/sort/nocaseglob/dotglob/nullglob/failglob-aware); completes extglob (string + pathname) |
| v92       | bare-word `[[ word ]]` truthiness (M-14c): a lone operand inside `[[ ]]` is a non-empty-string test (`[[ word ]]` ≡ `[[ -n word ]]`); closes a v30 M-14 gap that cascaded into `unexpected else/fi/}` errors when sourcing bash-completion |
| v93       | `$`-form expansion inside `(( ))`/`$(( ))`/arith-`for` (M-88, expand-then-parse): `$#`/`${…}`/`$(…)`/`$@`/`$1`/positional params now expand before arithmetic eval (the dominant bash-completion blocker, `(($# == 2))`); quote removal honored, malformed arith errors at eval time; `declare -f`/`-F` silent-on-missing |
| v94       | line numbers in sourced-script syntax errors (`FILE: line N: syntax error`); diagnostics iteration (no M-flip) |
| v95       | `${!var}` indirect parameter expansion (M-91): bare `${!ref}`, alphabetic + numeric-positional source (`${!2}`), modifier composition, array-element source; new `indirect` field + `expand_indirect` helper; clears the entire bash-completion `${!…}` error cascade. Bundled: `[[ ]]` integer comparison treats an empty operand as `0` (M-14). 20th bash-diff harness; prefix-name `${!prefix@}`/`${!prefix*}` deferred (M-92) |
| v96       | `${var@OP}` scalar parameter transforms (M-86 scalar subset): `@P` (prompt-expand), `@Q` (shell-quote; unset→empty), `@U`/`@L`/`@u` (case), `@E` (backslash-escape expand) via a new `ParamModifier::Transform`/`TransformOp` reusing `expand_prompt`/`case_modify`/`decode_ansi_c_escapes`/`shell_quote`; clears oh-my-posh's `${prompt@P}` block. 21st bash-diff harness; array/attribute forms `@A`/`@K`/`@k`/`@a` deferred (M-93) |
| v97       | redirections on compound commands (M-94): a redirect (`<`, `<<`, `<<<`, `>`, `>>`, `2>`, `2>>`, `&>`, `&>>`, `>&N`, `2>&N`) on `while`/`until`/`for`/`if`/`case`/`{ }`/`( )` subshell/`select`/C-style `for`/`(( ))`/`[[ ]]` now works (was `unexpected token after command`); new `Command::Redirected` wrapper + factored `parse_trailing_redirects` + fd-level `CompoundRedirectScope` RAII guard (`dup2` onto 0/1/2, restored on drop); compound stdout-redirect inside `$(…)` diverts correctly. Parses nvm.sh past its `done <<EOF` (line 567→1192). 22nd bash-diff harness; general `N>file` (N∉{0,1,2}) and process substitution out of scope |
| v98       | `&` as an async list separator (M-95): `&` now backgrounds the preceding and-or group and continues the list — `a & b`, `cmd & cmd2 &`, `for … do cmd & done`, `if … then cmd & fi`, `{ cmd & cmd2; }`, `( a & b )`, with bash-correct grouping (`a && b &` backgrounds the whole `a && b`); was `'&' not allowed here` as a separator, and the subshell parser silently ran `( a & b )`'s `a` in the FOREGROUND. New `Connector::Amp` on the flat `Sequence` (AST kept flat); group-aware executor (`partition_into_groups` + extracted `run_andor_group`; backgrounded group → synthetic `Subshell` via `run_background_subshell`). Bundled: async job notifications suppressed in non-interactive mode (bash-match). Parses nvm.sh past line 1192. 23rd bash-diff harness; nested and-or AST rewrite deferred (M-96); `&`-in-`$(…)` capture edges (L-18) |
| v99       | `command CMD [args]` bare form (M-85): runs CMD suppressing shell-FUNCTION/alias lookup — builtins and `$PATH` commands still resolve (`command echo` → echo builtin; a user function named `sort` is bypassed by `command sort` → the real external). Interception in `run_exec_single` scans leading flags (`-v`/`-V` → existing introspection; `-p` accepted; `--`; bad flag → rc 2), rewrites `resolved.program`/`args` + sets `bypass_functions` (collapses `command command …`), and gates ONLY the function-lookup arm on `!bypass_functions`; inline-assignment scope honors it too (bypassed function's assignment is temporary). Pre-resolve interception handles `command <declaration-builtin>` (`command export X=1` works; `command declare -a a=(…)` no longer panics). Drives `~/.nvm/nvm.sh`'s 167 `command sort`/`sed`/… uses — bare-form runtime errors gone. 24th bash-diff harness; `-p` live-`$PATH` / `command declare -a` superset / function-named-`command` edges (L-19) |
| v100      | subshell/compound-headed pipeline in any sequence position (M-11a): `( … ) \| cmd`, `{ …; } \| cmd`, `if…fi \| cmd`, `for…done \| cmd` etc. now parse after `;`/`&&`/`\|\|`/`&` and inside compound/function bodies — not just as the first command of a line (previously the `\|` after the `)`/`}`/`fi` in a non-first position was left unconsumed → `unexpected token after command`). New `parse_command_then_pipeline` helper (factored from the first-position pipeline-wrap, inlined identically in `parse_sequence`/`parse_subshell_sequence`) applied uniformly at the first + every rest-connector position; parser-only (a `Pipeline` with a compound first stage already executed). Bundled negation-hoist fix (`! ( false ) \| cat` now negates the whole pipeline). Drives `~/.nvm/nvm.sh`'s `nvm_list_aliases` (`( for … done; wait ) \| command sort` after other statements). 25th bash-diff harness |

## Build and run

```sh
cargo build --release
cargo run                # interactive REPL
cargo test               # full test suite (1000+ tests)
```

## Features

**Syntax:**
`cmd a b c`, `cmd1 ; cmd2`, `cmd1 && cmd2`, `cmd1 || cmd2`, `cmd1 | cmd2`,
`cmd > out`, `cmd < in`, `cmd >> out`, `cmd 2> err`, `cmd &`,
`echo "$VAR"`, `echo $(date)`, `NAME=value cmd`, `cd ~`, `ls ~/dir`,
`cd ~-`, `PATH=~/bin:~/lib`, `ls *.txt`, `echo [ab].rs`, `echo $((2+3))`, `echo ${X:-default}`, `echo ${f##*/}`.

**Builtins:**
`cd`, `pwd`, `echo`, `exit`, `export`, `unset`, `jobs`, `wait`, `fg`, `bg`,
`kill`, `disown`, `history`, `test`, `[`, `break`, `continue`.

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
Pattern substitution `${var/pat/repl}` (v32) replaces the first match;
`${var//pat/repl}` replaces all; `${var/#pat/repl}` and
`${var/%pat/repl}` anchor at start or end; the replacement is optional
(missing → empty); `\/` escapes a literal slash in the pattern.
Substring `${var:off:len}` and case modification are not yet implemented.

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

**Tab completion (v14):**
Tab completes command names (builtins and `$PATH` executables) in
command position, filenames and paths in argument position
(directories shown with a trailing `/`), and variable names after
`$`/`${`. The first Tab fills in the longest common prefix; a second
Tab lists all candidates. Filenames with shell-special characters are
backslash-escaped when inserted; a leading `~/` is expanded before
the directory is scanned; hidden files appear only when the typed
prefix begins with `.`. Per-command argument completion and `~user`
completion are not implemented.

**Conditionals (v16):**
`test EXPR` and `[ EXPR ]` evaluate file tests (`-e`/`-f`/`-d`/
`-r`/`-w`/`-x`/`-s`/`-L`), string tests (`-z`/`-n`/`=`/`!=`),
and integer comparisons (`-eq`/`-ne`/`-lt`/`-le`/`-gt`/`-ge`),
with `!` negation. Exit status is 0 (true), 1 (false), or 2
(usage error). The `-a`/`-o`/`( )` combinators and `[[ ]]` are
not implemented; `if` is a separate iteration.

**`if` control flow (v17):**
`if LIST; then LIST; [elif LIST; then LIST;]... [else LIST;] fi`
runs the `then` body when the condition's exit status is 0, an
`elif` body when its condition succeeds, or the `else` body. An `if`
is a compound command at the sequence level: it composes with `;`,
`&&`, `||`, nests inside branch bodies, and can be followed by more
commands. `if` inside a `|` pipeline and backgrounding a whole `if` are not yet implemented.

**`while` / `until` loops (v18):**
`while LIST; do LIST; done` runs the body while the condition's exit
status is 0; `until` runs it while the condition is non-zero. `break`
exits the innermost loop and `continue` skips to its next iteration.
An infinite `while true; do …; done` is interruptible with Ctrl-C.
Loops are sequence-level compound commands — they compose with `;`,
`&&`, `||` and nest. `break N` / `continue N` exit or continue the Nth
enclosing loop (v79).

**Multi-line input (v19):**
A command can span several input lines. The REPL reads continuation
lines — showing a `> ` prompt — until the typed text forms a complete
command: an unterminated `if`/`while`/`until`, an open quote or
expansion (`'`, `"`, `` ` ``, `$(`, `${`, `$((`), a pending operator
(`|`, `&&`, `||`), or a line ending in a backslash all carry over onto
the next line. `if`/`while`/`until` can therefore be written across
multiple lines, the way they appear in scripts. Ctrl-C at the `> `
prompt discards the partial command; an EOF mid-command is a syntax
error. A multi-line command is stored in history collapsed onto one
line.

**`for` loops (v20):**
`for NAME in WORD...; do LIST; done` runs the body once per word, with
`NAME` set to each word in turn. The word list is expanded once before
the loop — variables, command substitution, globs, and word-splitting
all apply, exactly as for command arguments (`for f in *.txt`, `for x
in $list`, `for n in $(seq 3)`). `break`/`continue` and multi-line form
work as for `while`. The no-`in` form (`for NAME; do … done`) iterates
`"$@"` — the current positional parameters — matching bash (M-24a, v81).
An explicit empty `in` (`for x in ; …`) still iterates nothing.
After the loop `NAME` keeps its last value. C-style `for ((init;cond;step))`
is also supported (v78).

**`case` statements (v21):**
`case WORD in PATTERN) LIST ;; … esac` matches the expanded subject
against each clause's glob patterns (`*`, `?`, `[…]`), runs the first
matching clause's body, and stops. Patterns may be `|`-alternated and
may carry an optional leading `(`. A quoted metacharacter matches
literally (`"*"` matches a literal `*`). All three terminators are
supported: `;;` (done), `;&` (fall through into the next clause's
body), `;;&` (keep testing later patterns). Clause bodies may be empty
and the final `;;` may be omitted (a separator before `esac` is still
required, as for `fi`/`done`). `break`/`continue` inside a body target
the enclosing loop — `case` is not a loop. Multi-line form works as for
the other compound commands. Adding `case` made `(`, `)`, `;;`, `;&`,
`;;&` lexer tokens; an unquoted `(` or `)` is now a shell metacharacter
(quote it to keep it literal: `"("`/`')'`).

**`select` loops (v81):**
`select NAME [in WORDS ...]; do COMMANDS; done` presents a numbered menu
of WORDS on stderr, prints the `PS3` prompt (`#? ` by default), and reads
a line into `REPLY`. `NAME` is set to the chosen word (or empty if the
reply is not a valid item number) and `COMMANDS` run; the loop repeats
until EOF or `break`. A blank line at the prompt reprints the menu without
running the body. The no-`in` form iterates `"$@"`, matching bash.

**Functions (v22):**
`name() compound-command` defines a function (the canonical body is a
brace group `{ … }`, but any compound — `if`/`while`/`for`/`case`/
`{ … }` — works). Calling `name arg1 arg2 …` runs the body with the
positional parameters `$1`, `$2`, … set to the call's arguments and
restored afterward. `$@` and `$*` give all args (`"$@"` preserves each
as its own field — the only construct that produces multiple fields
when quoted; `"$*"` joins them with a space). `$#` is the argument
count. `${10}` and higher use the braced form. `return [N]` exits a
function early with status `N` (defaulting to `$?`). A function
shadows any builtin except the flow-control set (`return`/`exit`/
`break`/`continue`), so `cd() { … }` works but `return() { … }` is
unreachable. `break`/`continue` inside a function correctly error as
"only meaningful in a loop" — the function boundary resets loop depth (v79,
matching bash). Redirections on a function call
(`func > file`) are not implemented. v22 also adds the standalone
brace group `{ list; }` (runs in the current shell — no subshell
isolation).

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
Interactive features (tab completion, history recall, Ctrl-C) are
covered by a PTY-driven golden-path suite in `tests/pty_interactive.rs`
using the `expectrl` crate; it skips gracefully where no PTY is
available.

## Dependencies

- `rustyline` — line editing
- `signal-hook` — SIGINT, SIGCHLD
- `libc` — `waitpid`, `setpgid`, `killpg`, `kill`, `tcsetpgrp`, `signal`
- `expectrl` — PTY-driven interactive tests (dev-dependency)

## License

Personal learning project; no license declared.
