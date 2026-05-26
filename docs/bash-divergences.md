# huck vs bash 5.x — Divergence Reference

**Last updated:** 2026-05-26 (after v26 special parameters).

This is the running audit of where huck differs from bash 5.x. Update each
entry's **Status** as fixes land. Reference an ID (e.g. `B-01`) in commit
messages so the doc stays in sync.

## How to read

- Each entry has an ID (`B-` bugs, `M-` missing features, `I-` intentional,
  `L-` low-impact), status, severity, the two behaviours, and (when known)
  the fix location.
- **Status**: `open` (not addressed), `fixed (sha)` (now matches bash),
  `intentional` (deliberate divergence we're keeping), `deferred (vN)`
  (planned for a specific future iteration).
- **Severity**: `high` (likely to surprise users / break scripts), `medium`
  (rare but real), `low` (cosmetic / edge case).

## Summary

| Tier | Count | Notes |
| --- | --- | --- |
| Bugs (Tier 1) | 10 | Things to fix (B-10 fixed 2026-05-26) |
| Missing features (Tier 2) | 56 | Bash-compat backlog (M-10 fixed by v25; M-01/02/03 fixed by v26) |
| Intentional (Tier 3) | 10 | Deliberate divergences we're keeping (I-16 fixed by v25) |
| Low-impact (Tier 4) | 7 | Edge cases, cosmetic |

---

## Tier 1: Bugs

huck behaves wrong without a design reason; should be fixed.

### B-01: `#` comments not supported
- **Status**: fixed (2026-05-23)
- **Severity**: high
- **huck (was)**: `echo foo # comment` ran with `#` and `comment` as literal args.
- **bash**: an unquoted `#` that begins a word starts a comment to end-of-line.
- **Fix**: `src/lexer.rs` `tokenize` — `'#' if !has_token` arm consumes through to (but not including) the next newline. `#` mid-word, inside quotes, or after a backslash stays literal because those paths never reach this arm.

### B-02: `` \` `` inside `"…"` not honored
- **Status**: fixed (2026-05-23)
- **Severity**: medium
- **huck (was)**: `"\`"` produced two characters (`\` then `` ` ``); the escape set was `"`, `\`, `$` only.
- **bash**: `\`` inside `"…"` is a literal backtick.
- **Fix**: `src/lexer.rs` — added `` ` `` to the double-quote escape set so the escape set now matches POSIX (`"`, `\`, `$`, `` ` ``; newline already handled at continuation time).

### B-03: backslash-newline only detected at end-of-buffer
- **Status**: fixed (2026-05-24)
- **Severity**: low (rarely hit in practice)
- **huck (was)**: only the v19 continuation classifier handled `\<NL>` (trailing, end-of-buffer); an embedded `\<NL>` inside an already-multi-line buffer fell through to the lexer and was emitted as a literal `\n` Literal.
- **bash**: `\<NL>` anywhere in a word is a line continuation. Inside `"..."` too (POSIX 2.2.3); inside `'...'` it stays literal.
- **Fix**: `src/lexer.rs` — outside-quote and inside-double-quote backslash arms each grow a `Some('\n') => {}` branch that deletes both characters.

### B-04: completion context doesn't reset after compound-command keywords
- **Status**: fixed (2026-05-23)
- **Severity**: medium
- **huck (was)**: `analyze()` reset `is_command_pos` only on `;`/`|`/`&`. After `then`/`do`/`else`/etc., tab offered filenames.
- **bash**: completion is keyword-aware.
- **Fix**: `src/completion.rs` — new `is_compound_keyword` helper recognises `then`/`do`/`else`/`elif`/`fi`/`done`/`esac`/`{`/`}`; the whitespace branch keeps `is_command_pos` true after one of these.

### B-05: `exit N` doesn't mask to 8 bits
- **Status**: fixed (2026-05-23)
- **Severity**: medium
- **huck**: `exit 300` returned `ExecOutcome::Exit(300)` internally; the OS still masked at process-exit time, but the unmasked value would have surfaced in any future code path that observed the exit status before the process died (e.g. command substitution).
- **bash**: POSIX requires exit status modulo 256; `exit 300` reports 44.
- **Fix**: `src/builtins.rs` `builtin_exit` applies `code.rem_euclid(256)`.

### B-06: `echo -n` / `-e` / `-E` not supported
- **Status**: fixed (2026-05-23)
- **Severity**: high
- **huck (was)**: `echo -n hello` printed `-n hello\n` literally; `-e` and `-E` similarly passed as args.
- **bash**: `-n` suppresses trailing newline; `-e` enables escape interpretation; `-E` disables it (default). Combined like `-ne`.
- **Fix**: `src/builtins.rs` — `parse_echo_flags` consumes leading flag groups; `process_echo_escapes` handles `\a \b \c \e \f \n \r \t \v \\ \0NNN \xHH`; unknown escapes keep the backslash (bash echo behaviour).

### B-07: `expand_assignment` reads `$?` live (latent v21 issue)
- **Status**: fixed (2026-05-23)
- **Severity**: medium
- **huck (was)**: `NAME=$(false)$?` read the post-substitution `$?` (`1`) instead of the pre-assignment value. v21 fixed this for `expand_pattern` only.
- **bash**: `$?` inside an assignment RHS reflects the status from before the assignment.
- **Fix**: `src/expand.rs` `expand_assignment` snapshots `shell.last_status()` at entry. All three expansion entry points (`expand`, `expand_assignment`, `expand_pattern`) now agree.

### B-08: `[N] Done` notification omits status for non-zero exits
- **Status**: fixed (2026-05-23)
- **Severity**: medium
- **huck (was)**: a synchronous synthetic builtin-background "done" notification always printed bare `[N] Done`, regardless of exit status; a second (correctly-formatted) notification then fired at the next `reap_and_notify` pass — duplicate output.
- **bash**: prints `[N]+ Exit N cmd &` when the background command exited non-zero.
- **Fix**: `src/executor.rs` `run_background_sequence` — the pure-builtin path now calls `crate::jobs::reap_and_notify(shell)` after `add_synthetic_done`. The job is formatted via `notification_line` (which uses `render_state` → "Done" vs "Exit N") AND marked notified, so `remove_notified` drops it on the same sweep. The defensive all-Assign fallback path was migrated the same way.

### B-09: `run_multi_stage` foreground wait loop iterates per-pid
- **Status**: fixed (2026-05-24)
- **Severity**: medium
- **huck (was)**: the foreground pipeline wait loop iterated `stages` one PID at a time with per-pid `waitpid`. If one stage stopped while the loop was blocked on a sibling (worst case: a producer/consumer deadlock pair), huck would wedge.
- **bash**: foreground wait targets the whole process group.
- **Fix**: `src/executor.rs` — new `wait_pgrp_pipeline` helper calls `waitpid(-pgid, …, WUNTRACED)` in a loop until every process stage is reaped (or any one stops). EINTR is retried; ECHILD bails with status 1 for remaining slots. Pipeline exit status is the last stage's status per POSIX. PTY regression test in `tests/pty_interactive.rs`.

### B-10: history expansion intercepted `$!` inside double quotes
- **Status**: fixed (2026-05-26)
- **Severity**: medium
- **huck (was)**: `echo "[$!]"` triggered "event not found" because the history-expansion scanner saw `!]` as a `!event` prefix-search. The scanner fired before the lexer and didn't suppress `!` when preceded by `$`.
- **bash**: `$!` is recognised as a special parameter reference; history expansion never sees the `!` because of the preceding `$`.
- **Fix**: `src/history.rs::scan()` — added a guard so `!` preceded by `$` is never treated as a history event.
- **Why this hit during v26**: surfaced when adding tests for the new `$!` special parameter; `[$!]` failed before the lexer got a chance.

---

## Tier 2: Missing bash features

Bash features huck doesn't implement. Listed roughly by impact within each
group.

### Special parameters

- **M-01: `$0`** — `[fixed (2026-05-26)]` high. Now supported: top-level returns argv[0] (typically `huck` or the full path); inside a function call, returns the function name (bash semantics).
- **M-02: `$$`** — `[fixed (2026-05-26)]` high. Now supported: returns the shell's PID, cached at startup. Subshells (v25) inherit the cached value via fork — `$$` is stable across the subshell boundary, matching bash.
- **M-03: `$!`** — `[fixed (2026-05-26)]` high. Now supported: after each backgrounded pipeline (`cmd &`), `$!` returns the LAST stage's PID per POSIX. Empty string until first background.
- **M-60: `${#1}` (length of positional)** — `[open]` low. huck: `${#name}` requires non-digit name; rejects `${#1}` as `InvalidVarName`. bash: returns the length of `$1`.

### Functions & scoping

- **M-04: Inline assignments `VAR=val cmd`** — `[fixed (2026-05-24)]` high. Now supported: leading `NAME=value` words on a simple command are applied left-to-right with the export flag, then restored (for external commands and regular builtins) or persisted (for special builtins, functions, and command-less assignment lists) per POSIX 2.14 / 2.9.1.
- **M-05: IFS not configurable** — `[deferred]` high. huck: word-splitting hardcoded to ASCII whitespace. bash: any `IFS` value governs splitting.
- **M-06: `local` / `typeset`** — `[deferred]` high. huck: no function-scoped variables. bash: `local` declares scoped vars.
- **M-07: `shift [N]`** — `[deferred]` medium. huck: not implemented. bash: removes the first N positional args.
- **M-08: `set --` and `set` flags** — `[deferred]` medium. huck: not implemented. bash: `set -- a b c` resets positionals; `set -o`/`-e`/`-u`/`-x`/`pipefail` set shell options.
- **M-09: `function name { … }` keyword form** — `[deferred]` medium. huck: only the POSIX `name() …` form. bash: also accepts the `function` keyword form.
- **M-10: Functions as pipeline stages** — `[fixed (2026-05-26)]` high. Pipeline stages of any Command type — simple commands, builtins, function calls, `if`/`while`/`for`/`case`/`{ }`, and function definitions — now run in forked subshells per POSIX 2.12. The parent's function table is inherited across the fork so `cmd | myfunc` finds and runs `myfunc`.

### Compound commands

- **M-11: Subshells `( list )`** — `[deferred]` high. huck: bare `(`/`)` is now a parse error (v21). bash: runs the list in a forked subshell with isolated state.
- **M-12: Here-documents `<<EOF`** — `[fixed (2026-05-24)]` high. Now supported: `<<DELIM` (expanding), `<<'DELIM'` (literal), `<<-DELIM` (tab-strip), composable; multiple here-docs per command; per-stage in pipelines; full POSIX expansion (`$var`, `${var}`, `$(cmd)`, backticks, `\$`, `\\`, `` \` ``).
- **M-13: Here-strings `<<<word`** — `[deferred]` medium. huck: parse error. bash: the expanded word becomes stdin.
- **M-14: `[[ … ]]` extended test** — `[deferred]` high. huck: not implemented. bash: keyword test with pattern matching, `=~` regex, `<`/`>` string ordering, no word-splitting.
- **M-23: C-style `for ((init; cond; step))`** — `[deferred]` medium. huck: parse error. bash: standard counter loop.
- **M-24: `select` loops** — `[deferred]` medium. huck: not implemented. bash: interactive menu loop.

### Parameter expansion modifiers

- **M-15: `${var/pat/repl}` and `${var//pat/repl}`** — `[deferred]` high. huck: `InvalidBraceModifier("/")`. bash: substitution.
- **M-16: `${var:off:len}` substring** — `[deferred]` high. huck: `InvalidBraceModifier(":N")`. bash: substring extraction.
- **M-17: `${var^^}` / `${var,,}` case modification** — `[deferred]` medium. huck: `InvalidBraceModifier("^")`. bash: upper/lower case.
- **M-58: `${var:?w}` doesn't abort non-interactive scripts** — `[open]` medium. huck: prints error, sets `$?` = 1, continues. bash: exits the script.

### Redirects

- **M-18: `2>&1` and `n>&m` fd-duplication** — `[deferred]` high. huck: parse error. bash: duplicates fds.
- **M-19: `&>file` combined redirect** — `[deferred]` medium. huck: parse error. bash: `>file 2>&1`.
- **M-20: `n<>file` read-write open** — `[deferred]` low. huck: not implemented. bash: opens fd for read+write.
- **M-21: `>|` and `noclobber`** — `[deferred]` low. huck: no `set -o noclobber`, no `>|`. bash: `noclobber` blocks overwriting; `>|` forces.
- **M-51: `|&` pipe stdout+stderr** — `[deferred]` low. huck: parse error. bash: shorthand for `2>&1 |`.

### Arithmetic (`$((…))`)

- **M-55: Bitwise operators `&`/`|`/`^`/`~`/`<<`/`>>`** — `[deferred]` high. huck: parse error. bash: full bitwise.
- **M-56: Assignment operators `=`/`+=`/`-=`/`*=`/`/=`/`%=`/`++`/`--`** — `[deferred]` medium. huck: bare `=` errors; `++`/`--` silently parse as double unary. bash: assignment mutates the shell var.
- **M-57: Non-decimal literals (`0x…`, `0…`, `N#…`)** — `[deferred]` medium. huck: hex/octal/base# all rejected. bash: full numeric base support.

### Quoting

- **M-28: `$'…'` ANSI-C quoting** — `[deferred]` high. huck: parses `$'\n'` as `$` + literal `\n` text. bash: processes C escapes.
- **M-29: `$"…"` locale quoting** — `[deferred]` low. huck: parses as `$` + double-quoted word. bash: gettext lookup.

### Job control

- **M-22: `trap` builtin** — `[deferred]` high. huck: not implemented. bash: signal handlers, EXIT/ERR/DEBUG/RETURN pseudo-signals.
- **M-37: `wait -n`** — `[deferred]` medium. huck: rejects `-n`. bash: waits for any one job to finish.
- **M-38: `wait` with multiple args** — `[deferred]` medium. huck: rejects more than one arg. bash: accepts a list.
- **M-39: `kill -l` (list signals)** — `[deferred]` medium. huck: rejects. bash: lists all signal names.
- **M-40: `kill -s SIGNAME`** — `[deferred]` medium. huck: only `-NAME` form (e.g. `-TERM`). bash: accepts `-s TERM`.
- **M-41: Limited signal name set** — `[deferred]` medium. huck: 15 names (no SEGV/ABRT/FPE/BUS/ILL/TRAP/…). bash: full platform signal set.
- **M-42: `kill` with negative PID** — `[deferred]` low. huck: rejects. bash: passes to `kill(2)` as a pgrp / wildcard target.
- **M-43: `disown -a`/`-r`/`-h`** — `[deferred]` medium. huck: only one bare-`%spec` arg. bash: flags + multiple args.
- **M-44: `disown` accepts bare PID** — `[deferred]` low. huck: requires `%spec`. bash: accepts PIDs.
- **M-45: `jobs -l`/`-p`/`-n`/`-r`/`-s`** — `[deferred]` medium. huck: rejects all args. bash: per-flag filtering / formatting.
- **M-50: `set -o pipefail` and `$PIPESTATUS`** — `[deferred]` medium. huck: pipe exit-status is the last stage; no per-stage status. bash: optional pipefail + array of per-stage statuses.
- **M-52: Backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`)** — `[deferred]` high. huck: parse error (`BackgroundedMultiPipelineSequence`). bash: runs the sequence in a subshell asynchronously.

### Builtins (other)

- **M-25: `test -a`/`-o`/`( )` combinators** — `[deferred]` high. huck: only POSIX 1-3 arg + `!`. bash: full chained expressions.
- **M-26: `test -v VAR`** — `[deferred]` medium. huck: not implemented. bash: tests if a variable is set.
- **M-27: Other `test` operators** (`-p`/`-S`/`-b`/`-c`/`-O`/`-G`/`-N`/`-k`/`-u`/`-g`/`-t`) — `[deferred]` medium. huck: only `-e`/`-f`/`-d`/`-r`/`-w`/`-x`/`-s`/`-L`. bash: full set.
- **M-30: `break N` / `continue N` / `return N` (level)** — `[deferred]` medium. huck: argument silently ignored beyond 1. bash: exits N enclosing loops.
- **M-31: `cd -`** — `[deferred]` high. huck: treats `-` as a path arg (fails). bash: cd to `$OLDPWD`. (Workaround: `cd ~-`.)
- **M-32: `cd -P` / `-L`** — `[deferred]` medium. huck: flags rejected. bash: physical/logical mode.
- **M-33: `pwd -P` / `-L`** — `[deferred]` low. huck: flags silently passed through. bash: physical/logical.
- **M-34: `hash` and command caching** — `[deferred]` low. huck: no caching, no `hash` builtin. bash: caches PATH lookups; `hash -r` clears.
- **M-35: `$PS2` as a variable** — `[deferred]` low. huck: continuation prompt hardcoded. bash: user-settable.
- **M-36: `complete` builtin / programmable completion** — `[deferred]` high. huck: only command/file/var completion. bash: full programmable API.
- **M-46: `history -d`/`-w`/`-r`/`-a` flags** — `[deferred]` low. huck: only `-c`. bash: full set.
- **M-47: `history N`** — `[deferred]` low. huck: rejects numeric arg. bash: prints last N entries.
- **M-48: `export -p`/`-n`** — `[deferred]` medium. huck: flags treated as variable names. bash: `-p` lists, `-n` unexports.
- **M-49: `unset -f`** — `[deferred]` medium. huck: only unsets variables. bash: `-f` unsets functions; `-v` is the explicit variable form.

### Globbing

- **M-53: `**` globstar** — `[deferred]` low. huck: `**` ≡ `*`. bash: `shopt -s globstar` makes `**` match across `/`.
- **M-54: POSIX bracket character classes `[[:alpha:]]` etc.** — `[deferred]` medium. huck: not supported by the `glob` crate. bash: full POSIX classes.

### History

- **M-59: `HISTSIZE` / `HISTFILESIZE` env vars** — `[deferred]` medium. huck: compile-time `HISTORY_MAX = 1000`. bash: reads env vars.

---

## Tier 3: Intentional divergences

Things huck deliberately does differently from bash. Document and keep.

### I-01: `cd` always sets the physical PWD
- **Status**: intentional
- **Severity**: medium
- **huck**: after `cd symlink`, `PWD` is the canonical path (`std::env::current_dir()`).
- **bash**: defaults to logical PWD (the path you typed, through symlinks).
- **Why**: simpler implementation; canonical paths are less surprising for cross-language tooling.

### I-02: `case` requires a separator before `esac`
- **Status**: intentional
- **Severity**: low
- **huck**: `case x in foo) echo hi esac` errors with `UnterminatedCase` (`esac` is eaten as an argument to `echo`).
- **bash**: same as huck — POSIX-strict; bash also requires a separator. (Documented here because the v21 spec example was initially wrong and was corrected.)
- **Why**: matches POSIX and `fi`/`done` precedent.

### I-03: REPL silently neutralizes stray `break`/`continue`/`return`
- **Status**: intentional
- **Severity**: low
- **huck**: a `return` (or `break`/`continue`) at the top-level prompt sets `$?` to 0 and continues.
- **bash**: prints an error and sets `$?` to 1.
- **Why**: deliberate friendly simplification.

### I-04: `for x; do` runs zero times (no `$@` at top level)
- **Status**: intentional (will revisit if `$@` ever gets a top-level source)
- **Severity**: medium
- **huck**: the no-`in` form iterates the empty current frame.
- **bash**: iterates `$@`, which at top level is the script's args.
- **Why**: huck has no script-file mode or `set --` (yet); the no-`in` form would always be empty otherwise.

### I-05: Multi-line commands collapse to one line in history
- **Status**: intentional
- **Severity**: low
- **huck**: v19 collapses a multi-line `if`/`for`/`{…}`/etc. into a single physical line using `;` / space / no-sep joiners. Lossy for quotes that span lines.
- **bash**: stores embedded newlines.
- **Why**: keeps the history file format one-entry-per-line.

### I-06: `(`/`)`/`{`/`}`/`;;`/`;&`/`;;&` are metacharacters
- **Status**: intentional
- **Severity**: low
- **huck**: unquoted `(` or `)` in arguments is a syntax error (v21); standalone `{`/`}` are keywords (v22).
- **bash**: same — `(` `)` and standalone `{`/`}` are all metacharacters.
- **Why**: required for `case`/subshell/brace-group recognition. Pre-v21 scripts using literal parens must quote them.

### I-07: Functions shadow regular builtins; control builtins are un-shadowable
- **Status**: intentional
- **Severity**: low
- **huck**: a user-defined `cd() { … }` overrides the builtin; `return`/`exit`/`break`/`continue` cannot be shadowed.
- **bash**: distinguishes "special" vs "regular" builtins per POSIX, with similar (but more nuanced) precedence.
- **Why**: shadowing fundamental flow control would let a user break the shell.

### I-11: EOF mid-command exits the shell with status 2
- **Status**: intentional (per v19 spec)
- **Severity**: medium
- **huck**: Ctrl-D while a partial command is pending → "syntax error: unexpected end of input", exit 2.
- **bash**: interactive Ctrl-D mid-buffer abandons the line and returns to the prompt.
- **Why**: v19 spec called this a deliberate simplification; revisit if it becomes painful.

### I-13: HISTFILE defaults to `~/.huck_history`
- **Status**: intentional
- **Severity**: low
- **huck/bash**: different shells, different defaults.

### I-15: Non-UTF8 command-sub output is lossy
- **Status**: intentional
- **Severity**: low
- **huck**: invalid UTF-8 from `$(cmd)` → `U+FFFD` replacement.
- **bash**: byte-faithful.

### I-16: Builtins in pipelines affect the parent shell
- **Status**: fixed (2026-05-26) by v25
- **Severity**: medium
- **huck (was)**: `cd /tmp | true` mutated the parent shell's cwd because builtin pipeline stages ran in-process in the parent.
- **bash**: every pipeline stage runs in a subshell; side effects are local.
- **Fix**: v25 rewrote `run_multi_stage` so every stage forks a subshell. See spec/plan dated 2026-05-25. (Informally referenced as "I-04" in the v25 spec/plan; the canonical ID here is I-16 since I-04 was already taken.)

---

## Tier 4: Low-impact / edge cases

- **L-01**: `~user` lookup capped at 16 KiB buffer. (Never hit in practice.)
- **L-02**: Glob sort order is byte-lexicographic, not `LC_COLLATE`-aware.
- **L-03**: Non-integer variable in `$((…))` errors instead of bash's "treat as recursive arith expression."
- **L-04**: `${#var}` counts Unicode chars; bash counts bytes (matches in UTF-8 locale).
- **L-05**: `[N] PID` spawn notification shows only the last pipeline stage's PID; bash shows all.
- **L-06**: `jobs` column width is fixed at 24; bash uses terminal width.
- **L-07**: `wait` polls (50ms) rather than blocking — small latency / minor CPU usage.

---

## Change log

- **2026-05-23**: Initial audit, baseline = v22 (commits up to `498d27d` merged + the `727cfcb` warning cleanup).
- **2026-05-23**: Quick-wins bug-fix batch shipped — B-01, B-02, B-04, B-05, B-06, B-07, B-08 all marked fixed.
- **2026-05-24**: Tier 1 finished — B-03 (backslash-newline mid-buffer line continuation) and B-09 (foreground pipeline pgrp wait) marked fixed. Baseline clippy warnings reduced from 22 to 0. Tier 1 is now empty (every "bugs" entry has Status=fixed).
- **2026-05-24**: M-04 (inline assignments) shipped as v23.
- **2026-05-24**: M-12 (here-documents) shipped as v24. Also reshapes ExecCommand.stdin from Option<Word> to Option<Redirect> so `<file`, `<<EOF`, and future `<<<word` share a uniform shape.
- **2026-05-26**: M-10 (functions and compound commands as pipeline stages) shipped as v25. Every pipeline stage now runs in a forked subshell per POSIX 2.12 — builtins, function calls, `if`/`while`/`for`/`case`/`{ }`, and function definitions all work as stages. Side-effect isolation is now correct: `cd /tmp | true` no longer mutates the parent's cwd.
- **2026-05-26**: I-16 added — the previously-undocumented "builtins in pipelines affect parent" divergence (informally "I-04" in the v25 spec) is resolved as a direct consequence of v25. Tracked here for discoverability. Compound-command redirects (`if …; fi <<EOF`) remain unimplemented (separate gap, not v25 scope).
- **2026-05-26**: M-01/02/03 (special parameters `$0`, `$$`, `$!`) shipped as v26. B-10 (history scanner intercepted `$!` inside double quotes) fixed as part of v26 testing — one-line guard in `src/history.rs::scan()`.
