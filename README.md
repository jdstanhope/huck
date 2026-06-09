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
| v101      | subshell / nested-arith inside command substitution (M-97): `$( (cmd) )`, `$( (a) \|\| b )`, `$(cmd \| (sub))`, `$( $((1+2)) )` now parse — were `syntax error in command substitution: unterminated '('`. One-arm lexer fix: `scan_paren_substitution` (the `$(…)` body scanner) now increments paren-depth on a bare `(` (a stale pre-v28 comment had it skip the increment), so a subshell `( … )` or nested `$((…))` no longer truncates the body at the inner `)`; transitively fixes `${x:-$( (a) )}` and array-literal `a=( "$( (x) )" )`. Parser/executor untouched. Drives `~/.nvm/nvm.sh`'s `nvm_resolve_alias` (`$( (pipeline) \|\| fallback )`) — nvm parses past line 1287, next gap at 2963. 26th bash-diff harness. Pre-existing edge (L-20): a `case`-pattern bare `)` inside `$(…)` still closes early |
| v102      | braced single-char special params `${-}`/`${?}`/`${$}`/`${!}` + modifiers (M-98): `${-}` (`$-`), `${?}` (`$?`), `${$}` (`$$`), `${!}` (`$!`) and modifier forms (`${-#*e}`, `${?:-x}`, `${$:+set}`) now parse/expand — were `syntax error: parameter expansion with empty name`. Lexer `read_braced_param_expansion` routes `-`/`?`/`$` through `dispatch_braced_modifier` + handles bare `${!}`→`$!`; `lookup_var` gains a `?` arm. Additive, parser/executor untouched. Drives `~/.nvm/nvm.sh`'s `nvm()` `${-#*e}` (line 2972) — the LAST syntax blocker, so nvm.sh now parses end-to-end with ZERO syntax errors (a separate source-time runtime HANG remains, a future gap). 27th bash-diff harness. `${!<modifier>}` stays the v95 indirect path (low edge) |
| v103      | `set -x` (xtrace) (M-08): `set -x`/`set +x`/`set -o xtrace`/`set +o xtrace` print each EXPANDED command to stderr prefixed by `$PS4` (default `+ `) BEFORE executing it; `x` shows in `$-`. Was rejected as "not yet supported". New `ShellOptions.xtrace` (mirrors v89 `verbose`); `dollar_dash_value` pushes `x`; `option_get`/`option_set` `xtrace` arms + `builtin_set` `-x`/`+x`; trace emitted in `run_exec_single` after expansion + the v99 interception, BEFORE dispatch — so a HANGING command is traced first (survives hang+SIGKILL). Functions/loops/subshells trace recursively. The diagnostic for v102's nvm source-time HANG: first finding is the hang is in nvm's TOP-LEVEL load code (~lines 413–440, NVM_DIR detection), not `nvm_resolve_alias`/`nvm_ls_current`; pinpointing the exact command is v104. Flat `$PS4`, no per-depth repeat / arg re-quote / inline-assign prefix / finer compound traces (L-21, intentional). 28th bash-diff harness |
| v104      | linear-time non-interactive script source reader (M-99): `run_sourced_contents` was O(n²) — it called `continuation::classify(&buf)` per physical line, and `classify` re-tokenizes + double-re-parses the WHOLE buffer each call, so a logical command of L lines cost O(L²). nvm.sh's entire 4619-line body is one `{ … }` brace group → a single logical command → catastrophic. **The v102/v103 "source-time hang" was THIS quadratic parse, not a runtime hang** (diagnosed via v103 `set -x`: traces grew super-linearly, a 40s timeout COMPLETED). Fix (additive, O(n)): tokenize the script ONCE, parse+execute one top-level command at a time. New `CharCursor` + `tokenize_with_offsets` (byte-offset sidecar, token output byte-identical) in `src/lexer.rs`; new `parse_one_unit` + `parse_sequence_opts(stop_at_top_newline)` in `src/command.rs` (`parse_sequence` a thin wrapper, callers byte-identical); re-lex the remainder only on a `shopt extglob` flip. Line numbers (v94), `set -v` (v89), errexit/`exit`/`return`/`set -u`, syntax-error report-and-continue all preserved from the offset sidecar. `classify` / `continuation` / the interactive `read_logical_command` reader UNTOUCHED (correct for incremental interactive input). **PAYOFF: `~/.nvm/nvm.sh` now sources in ~0.18s (previously did not finish).** 29th bash-diff harness + 13 integration tests (incl. a timing guard). Two boundary edges (trailing top-level `;`/`&` before a newline; post-error resync) → L-22, intentional |
| v105      | `[[ … =~ REGEX ]]` regex-operand lexing (M-100): inside `[[ ]]`, the right-hand operand of `=~` is now lexed as a SINGLE literal regex `Word` — `(`/`)`/`\|`/`((` are literal pattern text. Previously huck's `[[ ]]` lexing was context-free, so a regex `((` was grabbed as an arithmetic block (scan-to-EOF → `unterminated '((' arithmetic block`) and a regex `(` as shell grouping (→ `missing operand in '[[ ]]'`); real regexes like bash_completion's `[[ $option =~ (\[((no\|dont)-?)\]). ]]` (line 847) broke. Reached via v104's linear source reader. Lexer-only fix (`src/lexer.rs`): two new `tokenize_core` state fields — `dbracket_depth` (tracked on `[[`/`]]`) and `expect_regex` (set on an unquoted `=~` while inside `[[ ]]`); a new `scan_regex_operand` (modeled on `scan_extglob_group`) reads the operand as one `Word` with naive paren-depth tracking (`(`/`)`/`\|`/`((` literal; unquoted whitespace ends at depth 0, kept at depth > 0; `$var`/`${…}`/`$(…)`/quotes/`\`-escapes as in a normal word; no brace expansion / no extglob). Flows into the UNCHANGED `TestExpr::Regex { pattern: Word }` — parser/evaluator untouched. Drives bash_completion line 847 (+ its 8 other paren-regexes) — next bashrc gap moves to line 1232 (a command-substitution issue). 30th bash-diff harness. Pre-existing quoted-literal-regex divergence (L-23): a quoted `=~` substring is expanded as a word so its metacharacters stay active rather than matched literally as in bash |
| v106      | extglob inside command substitutions / array literals / `${…}` operands (M-101): `!(…)`/`@(…)`/`+(…)`/`*(…)`/`?(…)` now work inside `$(…)`/`` `…` ``, array-literal elements `a=(…)`, and `${…}` operands — not just at top level. Was `syntax error in command substitution: unexpected token after command` once extglob was on. Two parts: (1) lexer threading — the ~10 private helpers that recursively re-tokenize a nested body (`parse_substitution_body`, array-element/subscript/braced-operand helpers) lost the parent `LexerOptions` so they re-lexed `$()` bodies with extglob OFF (`!(…)`→`!`+`(…)`); fixed by threading `opts: LexerOptions` through them (`arith_string_to_word` keeps its `pub(crate)` sig, passes default opts → L-24). (2) reader interaction (v104) — the batch reader tokenized a whole chunk with the chunk-start extglob value; bash_completion enables extglob at line 45 but the chunk failed at line 1232 BEFORE line 45 ran, discarding everything up to it (20/81 funcs). Fixed: `tokenize_partial` returns the tokens before a lex error; the reader runs the complete units (running line 45's `shopt`), then re-lexes the truncated trailing unit with the now-current extglob. Bundled lexer fixes: `=~` regex operand on a `\`-newline continuation (`scan_regex_operand`, line 876) no longer empties on the continuation indentation; bare `{` in a `${var%%pat}` operand (`scan_braced_operand`, `${option%%[<{(]*}` lines 849/854) no longer breaks `}` matching (only `${` nests). **PAYOFF: `~/.nvm/.../bash-completion` now sources with ZERO errors and 82 functions defined (was 20); next `~/.bashrc` gap moves to a DIFFERENT file — `/usr/lib/git-core/git-sh-prompt` line 346.** 31st bash-diff harness, 2708 tests. New deferrals: L-24 (intentional) arith-nested command-sub doesn't inherit extglob; M-102 (deferred medium) array-literal element word-splitting `a=($(cmd))` makes ONE element not several (pre-existing, found in v106) |
| v107      | three small `~/.bashrc` gaps: M-103 (NEW) `[[ -o optname ]]` option test + M-79 `declare -g` + M-49 `unset -f`/`-v`. **M-103**: `[[ -o optname ]]` is the `[[ ]]` unary "is the `set -o` option `optname` enabled?" test — new `TestUnaryOp::OptEnabled` + `-o` in the `[[ ]]` unary table (`src/command.rs`), evaluated via `option_get(shell, name).unwrap_or(false)` (now `pub(crate)`); unknown/off → false; `test`/`[` keep the POSIX binary `-o`. **PAYOFF: fixes `/usr/lib/git-core/git-sh-prompt` line 406 `[[ -o PROMPT_SUBST ]]` and the `__git_ps1` define cascade** (huck previously failed `unterminated '[[ ]]'`, killing the whole function so its body's `local` statements ran at top level → a `local: can only be used in a function` flood; now `__git_ps1` defines cleanly). **M-79 (`declare -g`)**: `-g` accepted in both declare flag loops; when set it skips the local-scope snapshot + drops any outer snapshot for the name, so the write to `shell.vars` survives function exit (bash "declare = local without `-g`"); no-op at top level; composes with `-x`/`-i`/`-r` (`-l`/`-u`/`-n` still deferred). **M-49 (`unset -f`/`-v`)**: leading `-f`/`-v` flag scan — `-f` removes a function, `-v`/no-flag removes a variable; missing name → success (rc 0). 32nd bash-diff harness `tests/scripts/bashrc_builtin_gaps_diff_check.sh` (7 fragments) + 11 integration tests; full suite green, clippy clean |
| v108      | subshell-internal-pipeline tty deadlock (M-104): a multi-stage pipeline inside a subshell `( cmd1 \| cmd2 )` deadlocked whenever huck had a controlling terminal (interactive REPL, or `huck -c '( a \| b )'` under a pty) — `( echo hi \| cat )` printed `hi` then hung forever; `( echo hi )` (no inner pipeline) and `echo hi \| cat` (pipeline not in a subshell) were fine, and all three were fine in script mode / non-tty and in bash. Root cause: `fork_and_run_in_subshell` (`src/executor.rs`) reset `SIGTSTP`/`SIGTTIN`/`SIGTTOU` to `SIG_DFL` in the child, then the inner pipeline's `run_multi_stage` placed the stages in a NEW background process group (job-control path gated only on `matches!(sink, StdoutSink::Terminal)`, still true in a subshell under a tty), so the subshell's `wait` deadlocked. Fix (minimal): a new `Shell.in_subshell` bool set on the child side of the fork; `run_multi_stage`'s gate becomes `matches!(sink, StdoutSink::Terminal) && !shell.in_subshell`, so a subshell's inner pipeline takes the non-job-control path (stages stay in the subshell's process group), matching bash; top-level job control unchanged. **PAYOFF: this is the hang behind `source ~/.bashrc` — nvm's `nvm_resolve_alias` runs `$( ( nvm_alias … \| head -n1 \| tail -n1 ) \|\| nvm_echo )` at load, which now completes, so sourcing no longer hangs.** New pty regression test `tests/subshell_pipeline_pty.rs` + 4 non-pty equivalence tests; full suite green (incl. the 26-test `pty_interactive` job-control suite), clippy clean |
| v109      | toward zero-error `~/.bashrc`: 3 of the 5 `mise activate bash` leak types cleared — M-90 + M-89 + M-87. **M-90 (high, partial) — builtin stderr honors `2>`**: builtins now route `eprintln!` diagnostics through the command's fd 2 when a stderr redirect is present (`declare -p UNSET 2>/dev/null` is now silent; `2>>file`, bare `2>&1 \| cmd` work). New `BuiltinStderrGuard`/`prepare_builtin_stderr` RAII (`src/executor.rs`) mirroring `BuiltinStdinGuard` — `dup`/`dup2` the target onto fd 2 for the builtin's duration, restore on `Drop`; applied in both builtin arms; `2>&1` dups fd 1→fd 2 (Terminal-sink case). **Deferred to v110**: the combined `>/dev/null 2>&1` case (stdout also redirected → `2>&1` dup gated off, builtin stderr still leaks) + capture-mode `$(builtin 2>&1)` (L-25). **M-89 (low) — `export -a`**: `export` gained a leading-flag prelude consuming `-a`/`-p`/`-n`/`-f`/`--`; `-a` is a no-op accepted for bash-compat so `export -a chpwd_functions` → rc 0 (was `'-a': not a valid identifier`); `export -z` → invalid option rc 1; `export -a FOO=bar` keeps FOO scalar (bash makes an array — value same, attribute differs, out of scope). **M-87 (medium) — `${arr[@]±word}`**: the `+`/`-` (and `:+`/`:-`, on `[@]`/`[*]`, indexed + associative) set/unset modifiers on a whole array now work — `UseAlternate`/`UseDefault` arms in `expand_array_param`+`expand_assoc_param` replacing the catch-all reject; a whole array is "set" iff ≥1 element (empty `()` = unset); the alternate/default word is expanded field-preserving via `expand()` so the safe idiom `${arr[@]+"${arr[@]}"}` (mise's `${__MISE_FLAGS[@]+…}`) yields separate elements; `:=`/`:?` and per-element subst stay deferred. **Exposed M-105** (deferred v110): the pre-existing unquoted-`${x+alt}`-on-empty-array spurious-empty-field bug now injects an empty `''` arg into `mise hook-env`. **PAYOFF (partial): the 3 targeted leak types are gone; an end-to-end `mise activate` smoke still shows 2 residual errors (M-90 combined-redirect + M-105), tracked for v110.** 33rd bash-diff harness `tests/scripts/bashrc_zero_errors_diff_check.sh` (12 fragments) + 20 integration tests; full suite 2745 tests pass, clippy clean |
| v110      | **zero-error `mise activate`**: the last 2 leak types — M-90 combined `>file 2>&1` + M-105. **M-90 (high, completed) — builtin `>file 2>&1`**: v109 honored `2>file`/bare `2>&1` but gated the `2>&1` dup off whenever stdout was also redirected (`files.stdout.is_none()`), so `declare -p X >/dev/null 2>&1` (mise line 29) still leaked. `prepare_builtin_stderr`'s 2nd arg became `dup_target: Option<RawFd>`; both builtin arms select it — the redirected stdout **file's fd** (`as_raw_fd()`) for `>file 2>&1`, real fd 1 for bare `2>&1` under a Terminal sink, `None` for Capture (L-25). File writer + fd 2 share the open file description → both land in the file like bash. **M-105 (Tier-1 high) — unquoted `${x+alt}` spurious empty field**: `expand()`'s `ExpansionResult::Empty` arm set `has_emitted=true` unconditionally, so an unquoted `${x+alt}` expanding to nothing emitted a spurious empty field (`set -- ${u+X} a b; echo $#` → 3 vs bash 2), injecting an empty `''` arg into `mise hook-env`. Now quoted-aware (`if *quoted { has_emitted = true }`): unquoted empty vanishes, quoted empty still one field. Pre-existing scalar bug exposed for arrays by v109's M-87. **PAYOFF (the gate): sourcing `mise activate bash` (184 lines) through huck now emits 0 error lines (v109: 4).** 34th bash-diff harness `tests/scripts/mise_zero_errors_diff_check.sh` (10 fragments) + 9 integration tests; full suite 2754 tests pass, clippy clean. Residuals (low, deferred): capture-mode `$(builtin 2>&1)` (L-25), `2>&1 >out` ordering, converse-M-105 `${u+"$u"}` set-but-null |
| v111      | **`getopts` POSIX builtin (M-106)** — fixes `mise<TAB>` completion. `getopts optstring name [arg…]`: OPTIND/OPTARG/OPTERR, clustered short opts (`-abc`), `:`-suffixed args (attached `-bval` / separate `-b val`), `--` terminator, non-option stop, verbose (`illegal option -- c`, suppressed by `OPTERR=0`) + silent (leading `:` → `name='?'`/`':'`+`OPTARG`=char) error modes; no-`arg` form parses `$@`. Pure `getopts_step` state machine + `builtin_getopts` wrapper + a hidden within-word cursor (`Shell.getopts_sp`/`getopts_optind_cache`, reset on external `OPTIND` change, saved/reset/restored across `call_function` so a nested `getopts` can't corrupt a mid-cluster caller — bash isolates it; a flat cursor previously panicked). `name='?'` on every terminating call; `OPTARG` unset except a real arg / silent char — byte-identical to bash. **PAYOFF: clears the `mise<TAB>` `command not found: getopts` AND the downstream `bash_completion: …: \`-n': unknown argument` cascade** (with `getopts` missing, `OPTIND` never advanced so `_get_comp_words_by_ref`'s next loop tripped on the unconsumed `-n`). Bundled: made the brittle `tab_double_tab_lists` pty test order-independent (the new builtin reflowed the column-major completion listing). 35th harness `getopts_diff_check.sh` (13 fragments) + 11 unit + 10 integration tests; full suite 2775 tests pass, clippy clean. L-26: verbose-message `huck:`-vs-`$0` prefix (stderr only). `FUNCNAME` deferred (M-107) |
| v112      | **arithmetic comma operator (M-108)** — fixes the next `mise<TAB>` gap. `L , R` evaluates `L` (keeping side effects), discards its value, evaluates `R`, yields `R` — lowest precedence (below assignment: `a=1,2` is `(a=1),2`), left-associative; works in `(( ))`, `$(( ))`, parenthesized sub-exprs (`(1,2)+3`→5), and every C-style `for` clause. Mechanism (`src/arith.rs`): `ArithToken::Comma` + lexed `,`, `ArithExpr::Comma`, and a `parse_comma_expr` wrapper ABOVE the Pratt parser (`parse_expr(0)` then fold while peek==`,`) — no binding-power renumbering — wired into `arith::parse` (the single funnel for all 3 arith contexts) + the paren-group prefix; eval evaluates `l` for side effects then returns `eval(r)`. **PAYOFF: bash_completion's `__reassemble_comp_words_by_ref` runs `for ((i=0,j=0; …; i++,j++))` (mise<TAB>) — the `((: unexpected character: ','` error + the downstream `_upvars` `invalid option` cascade are gone.** Byte-identical to bash (value=last, side effects of all, comma-below-assignment, trailing/leading-comma parse error). Out of scope: comma in a ternary middle branch (deferred). 36th harness `arith_comma_diff_check.sh` (8 fragments) + 8 unit + 6 integration tests; full suite 2789 tests pass, clippy --all-targets clean |
| v113      | **`printf -v NAME[SUBSCRIPT]` array-element target (M-109)** — fixes the next `mise<TAB>` gap. `printf -v` now writes its formatted result into an array element — indexed (arith subscript) or associative (string key) — creating/promoting the array, same semantics as `NAME[SUBSCRIPT]=value` (readonly enforced). Was `printf: 'words[0]': not a valid identifier`. Mechanism (reuse, not reimplement): `builtin_printf` accepts a `name[sub]` `-v` target (via `split_name_subscript`, now `pub(crate)`) and routes the write through `apply_one_assignment` with an `AssignTarget::Indexed` — unquoted-literal subscript (arith-evaluated), quoted-literal value (stored verbatim); plain-name `-v` unchanged (`try_set`). **PAYOFF: bash_completion's `__reassemble_comp_words_by_ref` builds `words` with `printf -v "$2[i]" …` (mise<TAB>) — the `not a valid identifier` error + the downstream `_upvars` `invalid option` cascade are gone (REASSEMBLE_OK).** Byte-identical to bash (indexed/arith, associative, unset-promotion, overwrite, value-with-spaces verbatim, plain-name). Notes: `arr[@]`/`x[]` error like bash; element-case readonly message from `apply_one_assignment` (no `printf:` prefix, trivial divergence). 37th harness `printf_v_array_diff_check.sh` (8 fragments) + 6 integration tests; full suite 2795 tests pass, clippy clean |
| v114      | **alternate/default word quoting under an unquoted outer `${param+word}` (M-110)** — fixes the final `mise<TAB>` gap. With the OUTER `${p+word}`/`${p-word}` (`:+`/`:-`, scalar or array) **unquoted**, the substituted word's own quoting was lost: `a=(x "" y); ${a[@]+"${a[@]}"}` dropped the empty (2 vs bash 3), `${x+"a b"}` re-split (2 vs 1). Fix: new `ExpansionResult::Fields(Vec<Field>)` = `expand(word)` (per-char quoting kept); the UseAlternate(set)/UseDefault(unset) arms return it when `!quoted`, else the prior `Value`/`WordList` (quoted-outer path **untouched — zero regression**); `quoted` threaded into the scalar path via `expand_modifier_quoted`. The `expand()` consumer emits via `emit_split_field_quoted` (IFS-splits only UNQUOTED chars per the per-char mask, so `${x+a b}`→2 but `${x+"a b"}`→1, empties + `@`-boundaries survive); `expand_assignment()` joins. **PAYOFF: bash_completion's `__get_cword_at_cursor_by_ref` `${words+"${words[@]}"}` with `COMP_WORDS=(mise "")` dropped the trailing empty → desynced `_upvars -a${#words[@]}` → `bash_completion: : : invalid option` (×2); now gone (UPVARS_OK n=7).** Closes the converse-M-105 deferred in v110. Byte-identical to bash across the bisection + quoted-outer regression guards. 38th harness `alternate_word_quoting_diff_check.sh` (10 fragments) + 8 integration tests; full suite 2806 tests pass, clippy clean |
| v115      | **bare `local NAME` declares an UNSET local (M-111)** — was set-empty. A bare `local x` (no `=`) set `x` to `""`, so `[[ -v x ]]` was true and `${x-default}`/`${x+alt}` treated it as set; bash leaves a bare local UNSET. Fix (`src/builtins.rs`, both `local` paths): after the local-scope snapshot, `shell.unset(name)` for a bare name — gated on `!already_local` so a bare re-`local` of an already-local name preserves its value (`local x=v; local x` keeps `v`, matching bash); `local x=`/`x=val`/`-a`/`-A` unchanged; symmetric with the already-correct `declare x`. (Spec reviewer caught the re-`local` regression in the initial unconditional-unset version.) **PAYOFF (partial): bash_completion's `_get_comp_words_by_ref` `local … vcword vwords` + `[[ -v ]]` gates are now correct → the `mise<TAB>` `_upvars: : : invalid option` cascade is CLEARED.** A separate residual gap remains (**M-112, deferred**): `__get_cword_at_cursor_by_ref` builds an empty `words` array (huck `cword=0 nwords=0` vs bash `cword=1 nwords=2`), so completion is error-free but `prev`/`cword` are wrong — next triage. 39th harness `bare_local_unset_diff_check.sh` (11 fragments) + 8 integration tests; full suite 2814 tests pass, clippy clean |
| v116      | **`[^…]` negated bracket class in glob patterns (M-113)** — was treated as a literal `^` (the `glob` crate honors only `[!…]`; bash accepts both), inverting/breaking matching in every non-extglob context: `${v//[^0-9]/}` (abc123) gave `abc` not `123`, `case A in [^0-9])`→`other` not `letter`, `[[ A == [^0-9] ]]`→`N` not `Y`, `echo [^a]file`→`afile` not `bfile cfile`. Fix: new `translate_bracket_negation` helper (`src/glob_match.rs`) rewrites a class-leading `^`→`!` (bracket/escape-aware: honors `\[`, the literal-first-`]` rule, only the first char after an unescaped `[`; `Cow::Borrowed` when unchanged), applied before the `glob` call at 5 sites (`pe_pattern_matches`, `case`, `[[ == ]]`, completion `glob_match`, pathname `glob_with`); `[!…]`, literal-`^` (`[a^b]`), non-negated classes, and the extglob matcher (already handled `[^…]`) unchanged. **PAYOFF (partial): bash_completion's `${1//[^$COMP_WORDBREAKS]/}` exclude-set inversion is fixed — but `mise<TAB>` is STILL not functional; the smoke shows `: : invalid option` persists, now isolated to M-112 (the `_upvars` dynamic-scope upvar idiom), refined and left deferred.** Byte-identical to bash across `${}`/case/`[[`/pathname + `[!…]`/literal-`^`/non-negated regressions. 40th harness `bracket_negation_diff_check.sh` (12 fragments) + 8 integration tests (1 ignored: M-54 POSIX-class gap); full suite 2831 tests pass, clippy clean |
| v117      | **array-literal element field-expansion (M-112)** — `arr=(…)`/`arr+=(…)` elements weren't field-expanded: `arr=($s)`, `arr=($(cmd))`, `arr=("${w[@]}")`, `arr+=($s)` all collapsed to ONE element (bash splits into N). Fix: shared `expand_array_elements` routes BARE elements through `glob_expand_word` (the command-arg field+glob path — split/cmdsub/glob/`${arr[@]}`/`$@`, implicit index per produced field, fatal-PE checked); subscripted `[i]=val` stay single. `build_array_map` delegates (replace, start 0); the `a+=(…)` arm starts at max+1 and merges via the new `extend_indexed` (replacing the now-dead `append_array`). All `local`/`declare`/bare literals funnel through `apply_one_assignment`. **PAYOFF (PARTIAL, honest): the `mise<TAB>` `: : invalid option` is GONE — but `mise<TAB>` is NOT yet functional**; the smoke shows `_init_completion` rc=1, now blocked by **M-115** (multi-level `unset -v`/`eval` upvar dynamic-scope promotion across an intervening `local` shadow — the v118 target). Byte-identical to bash across split/cmdsub/glob/`[@]`/empties/mixed-index/append. 41st harness `array_literal_expansion_diff_check.sh` (13 fragments) + 15 integration tests; full suite 2846 tests pass, clippy clean. Logged M-114 (`eval x=(…)` argument array-literal panic) + M-115, both deferred |
| v118      | **`unset -v` dynamic-scope reveal/pop (M-115)** — `unset NAME` ignored `local_scopes`, so unsetting a variable local to an ENCLOSING function let that frame's snapshot clobber any later value on return; bash's upvar idiom (`unset -v NAME; eval NAME=val` writing into a caller across an intervening `local`) broke. Fix: new `Shell::unset_var` (used by the `unset` builtin's variable path) pops the nearest enclosing frame's snapshot + reveals its shadowed binding; current-fn-local/global unsets keep plain `vars.remove`; `unset -f`/`unset arr[i]`/readonly/internal callers untouched. Matches the 9 probed bash cases A–I. **PAYOFF: the bash_completion `_upvars`/`_init_completion` chain now WORKS — `_init_completion -n :` returns rc 0 with `cword=1 nwords=2 prev=mise` (first iteration to succeed); the only residual is a cosmetic `cur` trailing-space from M-54 (POSIX classes in `${var//pat}`), the next mise gap.** Byte-identical to bash across cases A–I. 42nd harness `unset_dynamic_scope_diff_check.sh` (13 fragments) + 9 integration tests; full suite 2859 tests pass, clippy clean |
| v119      | **POSIX bracket character classes in globs (M-54)** — `[[:alpha:]]`/`[[:digit:]]`/`[[:space:]]`/… (all 12) didn't work in any glob context (the `glob` crate lacks them), so `${s//[[:digit:]]/_}`, `case`/`[[ == [[:class:]] ]]`, completion, and pathname globbing all no-op'd. Fix: huck's own matcher (`glob_match.rs`) gained `ClassAtom::Posix` — `parse_class` recognizes `[:name:]` (unknown→matches-nothing) via ASCII/C-locale char predicates (`space` incl. `\v`, `print`=graphic-or-space); new `has_posix_class` routes class-bearing patterns through `extglob_match`/`extglob_pathname_expand` at all 5 sites, unconditional on the extglob shopt. **PAYOFF: closes the LAST `mise<TAB>` residual — `${cur//[[:space:]]/}` now clears the whitespace-only `cur`, so `_init_completion -n :` is fully byte-identical to bash (`cur=[]`).** Byte-identical across the 12 classes + negation + mixed + pathname. 43rd harness `posix_classes_diff_check.sh` (19 fragments) + integration tests; full suite 2872 tests pass, clippy clean |
| v120      | **`printf %q` + `set -f`/`noglob` (M-73 / M-08 sub-features)** — two POSIX-class gaps the `mise` `_mise` handler tripped over. `printf %q` shell-quotes args: backslash-style (`a b`→`a\ b`, glob/`$`/quote escaped), `$'…'` ANSI-C form for control chars, `''` for empty, `~`/`#` quoted only when leading; reuses a refactored `ansi_c_quote` shared with `${var@Q}`; honors width + `-v VAR` capture. `set -f`/`+f`/`set -o noglob` toggle a new `ShellOptions.noglob` field that `glob_expand_fields_opts` checks to suppress pathname expansion ONLY (word-splitting/`${//}`/`case`/`[[ == ]]` unaffected); `f` shows in `$-`, `[[ -o noglob ]]` reflects it. **PAYOFF: huck's `_mise` smoke no longer prints `printf: \`%q': invalid directive` or `set: noglob: not yet supported` — both errors cleared, byte-identical to bash (full `mise<TAB>` still needs the `mise` binary).** 44th harness `printf_q_noglob_diff_check.sh` (16 fragments) + integration tests; full suite 2881 tests pass, clippy clean |
| v121      | **completion job-control hang (M-116)** — interactive TAB completion wedged the whole shell whenever a completer ran an external command/pipeline (bash-completion's `_longopt` does `cmd --help 2>&1 \| while read …`): `call_completion_function` ran the completer with a `StdoutSink::Terminal` sink, so its subprocesses took the interactive job-control path (`setpgid` + `give_terminal_to`/`tcsetpgrp`) and handed the controlling terminal to a new process group while huck was mid-line-edit in raw mode → deadlock, no Ctrl-C escape (the v108/M-104 terminal-handoff hazard via the completion path). Fix: new `Shell.in_completion` flag, set for the dynamic extent of the completer call, gating both job-control deciders (`run_multi_stage`/`run_subprocess` gain `&& !shell.in_completion`) so completer subprocesses run foreground in huck's own process group — matching bash, which never job-controls completion functions. **PAYOFF: `ls -<TAB>`/`grep<TAB>`/etc. (any `_longopt`-completed command) are responsive again — verified STILL HANGS pre-fix → RESPONSIVE post-fix.** PTY regression test `completion_jobcontrol_pty.rs` (drives `ls -<TAB>` against real bash-completion, bounded-timeout non-hang assertion); `pty_interactive` (26) + `subshell_pipeline_pty` (2) stay green; full suite 2882 tests pass, clippy clean |

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
