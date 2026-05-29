# huck vs bash 5.x — Divergence Reference

**Last updated:** 2026-05-26 (after v28 subshells).

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
| Bugs (Tier 1) | 11 | Things to fix (all 11 fixed; B-11 fixed 2026-05-26) |
| Missing features (Tier 2) | 52 | Bash-compat backlog (M-10 fixed by v25; M-01/02/03 fixed by v26; M-13 fixed by v27; M-11 fixed by v28; M-18/19 fixed by v29; M-14 fixed by v30) |
| Intentional (Tier 3) | 10 | Deliberate divergences we're keeping (I-16 fixed by v25) |
| Low-impact (Tier 4) | 11 | Edge cases, cosmetic (L-08 added v29: redirect source-order divergence; L-09 added v30: regex-engine divergence; L-10 added v33: split-scanner limitation; L-11 added v39: `$'\xHH'` Unicode-vs-byte) |

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

### B-11: `$?` doesn't propagate across `;` separator
- **Status**: fixed (2026-05-26)
- **Severity**: medium
- **huck (was)**: `false; echo $?` printed `0` instead of `1`. Same for `[ 1 -eq 2 ]; echo $?`. Workaround was to use `\n` instead.
- **bash**: `$?` after `;` is the previous command's exit status.
- **Root cause**: `shell.last_status` was only refreshed in `src/shell.rs:75` after `process_line` returned. Within a sequence, `execute_sequence_body` held the per-command outcome in a local variable but never propagated it to `shell.last_status`, so the next command's expansion saw the stale pre-sequence value. The `&&`/`||` connectors worked by accident — they short-circuit on the local `status`, not on `$?`.
- **Fix**: `src/executor.rs::execute_sequence_body` — after each `run_command` call, when the outcome is `Continue(c)`, call `shell.set_last_status(c)` so subsequent commands in the sequence see the correct `$?`. `Exit`/`LoopBreak`/`LoopContinue`/`FunctionReturn` outcomes return early and are still handled by the top-level loop.

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
- **M-60: `${#1}` (length of positional)** — `[fixed v34]` low. Lexer `#` arm now accepts digit-only names (`${#1}`, `${#10}`) and the special `@`/`*` names that mean "count of positional args" (same as `${#}`). Length evaluator switched from `shell.get` to `shell.lookup_var` so digit names resolve through `positional_args`.

### Functions & scoping

- **M-04: Inline assignments `VAR=val cmd`** — `[fixed (2026-05-24)]` high. Now supported: leading `NAME=value` words on a simple command are applied left-to-right with the export flag, then restored (for external commands and regular builtins) or persisted (for special builtins, functions, and command-less assignment lists) per POSIX 2.14 / 2.9.1.
- **M-05: IFS not configurable** — `[deferred]` high. huck: word-splitting hardcoded to ASCII whitespace. bash: any `IFS` value governs splitting.
- **M-06: `local` / `typeset`** — `[deferred]` high. huck: no function-scoped variables. bash: `local` declares scoped vars.
- **M-07: `shift [N]`** — `[deferred]` medium. huck: not implemented. bash: removes the first N positional args.
- **M-08: `set --` and `set` flags** — `[deferred]` medium. huck: not implemented. bash: `set -- a b c` resets positionals; `set -o`/`-e`/`-u`/`-x`/`pipefail` set shell options.
- **M-09: `function name { … }` keyword form** — `[deferred]` medium. huck: only the POSIX `name() …` form. bash: also accepts the `function` keyword form.
- **M-10: Functions as pipeline stages** — `[fixed (2026-05-25)]` high. Pipeline stages of any Command type — simple commands, builtins, function calls, `if`/`while`/`for`/`case`/`{ }`, and function definitions — now run in forked subshells per POSIX 2.12. The parent's function table is inherited across the fork so `cmd | myfunc` finds and runs `myfunc`.

### Compound commands

- **M-11: Subshells `( list )`** — `[fixed (2026-05-26)]` high. Now supported: `(list)` runs the inner sequence in a forked subshell with isolated side effects. Reuses v25's fork machinery; the helper's child-side dispatch handles Subshell-as-pipeline-stage without a recursive double-fork. Top-level `(cmd)`, pipeline stages, backgrounded `(cmd) &`, nested `((cmd))`, and composition with heredocs/here-strings all work.
- **M-12: Here-documents `<<EOF`** — `[fixed (2026-05-24)]` high. Now supported: `<<DELIM` (expanding), `<<'DELIM'` (literal), `<<-DELIM` (tab-strip), composable; multiple here-docs per command; per-stage in pipelines; full POSIX expansion (`$var`, `${var}`, `$(cmd)`, backticks, `\$`, `\\`, `` \` ``).
- **M-13: Here-strings `<<<word`** — `[fixed (2026-05-26)]` medium. Now supported: `<<<word` feeds the expanded word (no split/glob) plus a trailing newline as stdin to the command. Reuses v24's deferred-expansion + stdin-pipe machinery — per-stage scoping, backgrounded forms, and pipeline composition all work.
- **M-14: `[[ … ]]` extended test** — `[fixed (2026-05-26)]` high. Now supported: pattern `==`/`!=` (RHS glob; quoted → literal), regex `=~` (via Rust `regex` crate — RE2-style; no lookbehind/lookahead), lexicographic `<`/`>` (byte-order; no LC_COLLATE), integer `-eq`/`-ne`/`-lt`/`-gt`/`-le`/`-ge`, file tests (`-f`/`-d`/`-r`/`-w`/`-x`/`-e`/`-s`/`-L`), string tests (`-n`/`-z`), combinators (`!`, `&&`, `||`, grouping `()`). No word-splitting or pathname expansion on operands per bash. Out of scope: `-v var` (var-set), `-nt`/`-ot`/`-ef` (file age/identity), bash arrays.
- **M-23: C-style `for ((init; cond; step))`** — `[deferred]` medium. huck: parse error. bash: standard counter loop.
- **M-24: `select` loops** — `[deferred]` medium. huck: not implemented. bash: interactive menu loop.

### Word expansion

- **M-61: Brace expansion (`{a,b,c}` / `{1..5}` / etc.)** — `[fixed v46]` medium. Comma lists, integer ranges (asc/desc, optional step, zero-padded), character ranges, prefix/suffix, nested braces, and Cartesian product across consecutive braces. Runs at the lexer stage before parameter / command / arith expansion. Quoted braces (`"{a,b}"`, `'{a,b}'`, `\{a,b\}`) are NOT expanded. Safety cap at 65,536 expansions per word; exceeding errors with `huck: syntax error: brace expansion: too many elements`.

### Parameter expansion modifiers

- **M-15: `${var/pat/repl}` and `${var//pat/repl}`** — `[fixed v32]` high. All six forms: `/`, `//`, `/#`, `/%`, plus empty-repl shortcut. Glob pattern engine; `\/` escapes literal slash in pattern. Bash-compat empty-pattern no-op + trailing-empty-match suppression for greedy patterns like `*`.
- **M-16: `${var:off:len}` substring** — `[fixed v33]` high. `${var:offset}` and `${var:offset:length}` for scalar vars and positional params (`${1:0:3}`). Offset/length are full arithmetic expressions via `arith::parse` + `arith::eval` (variable refs, `+`/`-`/`*`/`/`/`%`, parentheses). Char-counting (codepoints), bash 5.x edge-case semantics: negative offset counts from end, negative length counts from end, negative computed length errors. **Out of scope (still open)**: `${@:off:len}` and `${*:off:len}` array slicing on positional params.
- **M-17: `${var^^}` / `${var,,}` case modification** — `[fixed v37]` medium. All eight forms: `^^`/`^`/`,,`/`,` × bare/with-pattern. Pattern operand uses bash glob semantics (per-character match) via the existing `glob::Pattern` engine. Unicode-aware case mapping via Rust's `char::to_uppercase` / `char::to_lowercase` iterators — handles multi-char expansions like `ß`→`SS` correctly. Closes the parameter-expansion-modifier cluster started by v32 (substitute) / v33 (substring) / v34 (length + fatal PE).
- **M-58: `${var:?w}` doesn't abort non-interactive scripts** — `[fixed v34]` medium. `${var:?}` and substring-expression-<0 errors now return `ExpansionResult::Fatal { status: 1 }`, which the three `expand_*` functions stash on `Shell::pending_fatal_pe_error`. The executor's `resolve()` and `execute_sequence_body` peek-check the flag after each `expand()` and bail; the REPL drains via `take_pending_fatal_pe_error()` and (when `!is_interactive`) exits the shell. Bad-arith in substring offset/length stays non-fatal (matches bash).

### Redirects

- **M-18: fd-duplication `n>&m` and `&>file`** — `[fixed (2026-05-26)]` high. Now supported: `2>&1` (POSIX), `1>&2`, `&>file` (bash), `&>>file` (bash). Limited to fds 1 and 2 (arbitrary `n>&m` with n≠1,2 may parse but isn't claimed to work). `>&-` (close-fd) is out of scope. **Known divergence**: huck applies stdout-redirect before stderr-redirect (field-based AST loses source order); `cmd 2>&1 >file` (rare anti-pattern) produces both-to-file rather than bash's stderr-to-terminal. Canonical `>file 2>&1` works correctly. See L-08.
- **M-19: `&>file` combined redirect** — `[fixed (2026-05-26)]` medium. Covered by M-18 fix; `&>file` and `&>>file` are fully supported as bash shorthand for `>file 2>&1` / `>>file 2>&1`.
- **M-20: `n<>file` read-write open** — `[deferred]` low. huck: not implemented. bash: opens fd for read+write.
- **M-21: `>|` and `noclobber`** — `[deferred]` low. huck: no `set -o noclobber`, no `>|`. bash: `noclobber` blocks overwriting; `>|` forces.
- **M-51: `|&` pipe stdout+stderr** — `[deferred]` low. huck: parse error. bash: shorthand for `2>&1 |`.

### Arithmetic (`$((…))`)

- **M-55: Bitwise operators `&`/`|`/`^`/`~`/`<<`/`>>`** — `[fixed v38]` high. All six bitwise operators supported in `$((…))`. Shift counts must be in `[0, 64)` — out-of-range produces `ShiftCountOutOfRange` error (deliberate divergence from bash's C-undefined behavior). Bundled with `**` exponentiation (right-associative; negative exponents error). v38 also closes M-56 and M-57.
- **M-56: Assignment operators `=`/`+=`/`-=`/`*=`/`/=`/`%=`/`<<=`/`>>=`/`&=`/`^=`/`\|=`/`++`/`--`** — `[fixed v38]` medium. All 11 assignment operators + prefix/postfix `++`/`--` supported. `arith::eval` signature now takes `&mut Shell`. LHS must be a variable (enforced at parse time): `(a + b) = 5` rejects with parse error. v38 also closes M-55 and M-57.
- **M-57: Non-decimal literals (`0x…`, `0…`, `N#…`)** — `[fixed v38]` medium. Hex (`0x…` / `0X…`), octal (`0…` with digits 0-7), and base-N (`N#…` for 2 ≤ N ≤ 64) all supported. Bash's full digit alphabet (0-9, a-z, A-Z, @, _) implemented; for bases ≤ 36 letters are case-insensitive, for bases > 36 they're distinct. v38 also closes M-55 and M-56.

### Quoting

- **M-28: `$'…'` ANSI-C quoting** — `[fixed v39]` high. All 16 bash escapes: `\a`, `\b`, `\e`/`\E`, `\f`, `\n`, `\r`, `\t`, `\v`, `\\`, `\'`, `\"`, `\?`, `\nnn` (1-3 octal), `\xHH` (1-2 hex), `\uXXXX` (1-4 hex), `\UXXXXXXXX` (1-8 hex), `\cX` (control). Numeric escapes are interpreted as Unicode codepoints rather than raw bytes — see L-11. Unknown escapes preserve both the backslash and the following character (`$'\q'` → literal `\q`). Result emitted as a quoted Literal: no further expansion, no word splitting, no globbing. Implemented purely in the lexer (`read_dollar_expansion` + `read_ansi_c_quoted` + `decode_ansi_c_escape`).
- **M-29: `$"…"` locale quoting** — `[deferred]` low. huck: parses as `$` + double-quoted word. bash: gettext lookup.

### Job control

- **M-22: `trap` builtin** — `[fixed v36]` high. All four bash pseudo-signals (EXIT, ERR, DEBUG, RETURN) + 13 trappable real signals (huck's 15-name table minus KILL/STOP) now supported via `trap ACTION SIGNAL...`, `trap -p`, `trap -l`, `trap - SIGNAL`, `trap "" SIGNAL`. Action body stored as raw text, re-parsed via `process_line` at fire time (late variable binding). Async-signal-safe `Arc<AtomicU32>` bitmask delivery for real signals; per-event firing for pseudo-signals with `Shell::firing_trap` recursion guard. EXIT self-removes before firing. ERR fires after any non-zero command exit except inside `if`/`elif`/`while`/`until` conditions or on LHS of `||` chain (matches bash 5.x `set -e` rules). DEBUG fires before each simple command. RETURN fires after a function returns with `$?` set to the return value and the function's positional args still in scope. Subshell trap-clear matches POSIX. **Known limitations**: M-41 limited signal set still applies (no SEGV/ABRT/etc.); `trap "" SIGNAL` registers an empty custom handler rather than true SIG_IGN, so child processes after `exec` do NOT inherit the "ignore" disposition (matters for `trap '' PIPE; cmd | head`-style scripts); `$BASH_COMMAND` variable inside DEBUG/ERR/RETURN actions is not set (the action runs but the variable expands to empty); `! cmd` pipeline negation is not parsed by huck, so the bash `!` ERR exemption is moot.
- **M-37: `wait -n`** — `[fixed v40]` medium. `wait -n` waits for the next job to finish; with no positional args it considers all currently-Running jobs; with a target list it restricts to those. Bash 5.1 `-p VAR` flag also supported — captures the finished job's pgid (for `%spec` targets) or literal PID (for PID targets) into `$VAR`. Empty job list returns 127 immediately and clears `$VAR`. `-p` without `-n` is a usage error. `-f` and `-np` (combined-flag form) deferred.
- **M-38: `wait` with multiple args** — `[fixed v40]` medium. `wait PID1 PID2 …` and `wait %1 %2 …` (or mixed) now supported. Targets are waited for sequentially; exit status is the status of the LAST one waited. Unparseable args trigger usage error 2 BEFORE any waiting begins.
- **M-39: `kill -l` (list signals)** — `[fixed v41]` medium. All four bash forms supported: bare `kill -l` (4-column `NUM) NAME` listing of the 16 sendable signals = 14 trappable + KILL + STOP), `kill -l NAME` → number (case-insensitive, optional `SIG` prefix), `kill -l NUM` → name (no `SIG` prefix), `kill -l <status≥128>` → name via N-128 decode. Multiple args produce one decode per line, stopping at the first invalid arg. `kill -l` also fixed a latent bug where `kill -WINCH pid` was rejected because WINCH wasn't in the old hardcoded `signal_by_name` table.
- **M-40: `kill -s SIGNAME` / `kill -n SIGNUM`** — `[fixed v42]` medium. Both bash long-form flags supported: `kill -s NAME pid` (case-insensitive, optional `SIG` prefix) and `kill -n NUM pid` (NUM must be in `killable_signals()`). All four dispatch arms (`-s`, `-n`, `-<sig>`, default-SIGTERM) share a `send_signal_to_targets` helper. Existing `kill -TERM pid` / `kill -15 pid` / `kill pid` paths unchanged.
- **M-41: Limited signal name set** — `[deferred]` medium. huck: 15 names (no SEGV/ABRT/FPE/BUS/ILL/TRAP/…). bash: full platform signal set.
- **M-42: `kill` with negative PID** — `[deferred]` low. huck: rejects. bash: passes to `kill(2)` as a pgrp / wildcard target.
- **M-43: `disown -a`/`-r`/`-h` (and multi-arg)** — `[fixed v43]` medium. All three flags supported with combined forms (`-ah`, `-ar`, `-arh`). `-a` operates on all jobs (positional args ignored, bash-faithful); `-r` filters to Running only (bare `disown -r` operates on all running jobs, bash-faithful); `-h` marks the job for nohup (skipped by the shell's SIGHUP-on-exit broadcast) instead of removing it. Multi-arg `disown %1 %2 %3` now valid. Adds SIGHUP-on-exit behavior: clean exit (explicit `exit`, EOF, fatal-PE, ReadError) now sends SIGCONT + SIGHUP to every live unmarked job via `Shell::hangup_jobs`. **Behavior change**: previously huck never sent SIGHUP on exit; v43 aligns with bash's interactive default. Defensive patterns (`disown -h`, `nohup`) continue to work.
- **M-44: `disown` accepts bare PID** — `[fixed v44]` low. `disown 12345` now resolves the PID against every tracked job's `pids` list (including non-leader pipeline stages) and operates on the matching job as a whole. Existing `%spec` path unchanged. Unknown PIDs error with "no such job" + status 1.
- **M-45: `jobs -l`/`-p`/`-n`/`-r`/`-s` + positional `%spec`** — `[fixed v45]` medium. All five bash flags supported with combined forms (`-lr`, `-ln`, etc.). `-l` adds PIDs to the listing (bash-faithful multi-line format for pipelines: first stage carries `[N]<flag> <pid> <state> <command> &`, subsequent stages indented 5 spaces with PID only). `-p` prints only pgids, one per line, no decoration (overrides `-l` when both present, bash-compat). `-n` filters to jobs whose state changed since last query (consumes `Job.notified` flag; subsequent `jobs -n` after the same change shows nothing). `-r` filters to Running; `-s` filters to Stopped. Positional `%spec` args filter to specific jobs; combines AND with flag filters.
- **M-62: Extended job specs `%cmd` / `%?cmd`** — `[fixed v47]` medium. `%cmd` resolves to the unique job whose command starts with "cmd"; `%?cmd` resolves to the unique job whose command contains "cmd". Bash-faithful "ambiguous job spec" error when multiple jobs match (status 1). Empty pattern (`%?` alone) is a bad-spec parse error. `JobTable::resolve` signature changed from `Option<u32>` to `Result<u32, JobSpecResolveError>`; the single internal caller (`resolve_spec_or_error` in src/builtins.rs) updated. All builtins that take `%spec` args (fg, bg, wait, kill, disown, jobs) gain the new behavior transparently.
- **M-50: `set -o pipefail` and `$PIPESTATUS`** — `[deferred]` medium. huck: pipe exit-status is the last stage; no per-stage status. bash: optional pipefail + array of per-stage statuses.
- **M-52: Backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`)** — `[fixed v49]` high. Sequences using `&&`, `||`, `;`, or any combination can now end with `&` to background the whole sequence as a single job. Implementation forks once; child runs the entire sequence to completion (honoring `&&`/`||` short-circuit); parent registers a single-PID job and returns immediately. Equivalent to bash's `(cmd1 && cmd2) &` semantics. `jobs`, `wait %N`, `kill %N`, `disown %N` all work because the bg sequence registers as a single-PID job indistinguishable from `(cmd) &`.

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
- **M-63: Aliases** — `[fixed v48]` medium. `alias name=value` defines; bare `alias` lists; `alias name` shows one. `unalias name` removes; `unalias -a` clears all. Aliases expand at command position before parsing (after pipes, `&&`, `||`, `;`, `&`, `(`, newlines). Recursive expansion supported with per-input cycle protection (alias `ls='ls --color'` doesn't infinite-loop). Bash trailing-space rule honored: `alias sudo='sudo '` makes the next word also alias-eligible. Expansion only fires for interactive REPL input (`shell.is_interactive == true`) OR when the `HUCK_EXPAND_ALIASES` env var is set (escape hatch for scripts/tests); trap actions, function bodies, and ordinary non-interactive script execution are unchanged (matches bash defaults).
- **M-65: `shift` and `set --`** — `[fixed v50]` medium. `shift [N]` removes the first N positional parameters (N defaults to 1; negative or non-numeric → status 1; N > count → status 1). `set` with no args lists all shell variables in sorted `name='value'` form; `set --`, `set -- args`, and bare `set args` (no leading dash) all replace the current positional parameters. `set -e`/`set -x`/`set -u`/`set +o`/etc. (option flags) are explicitly rejected with status 2 and a clear "not yet supported in this version" message — these are a future iteration. Both join `is_special_builtin`'s set (POSIX classifies them special; inline assignments preceding them persist in the shell).
- **M-66: `source` and `.`** — `[fixed v51]` medium. Reads and executes commands from a file in the current shell context. Filename without `/` is searched in `$PATH` (bash-faithful). Optional arguments after the filename become positional parameters during the sourced execution, restored on return. `return N` inside a sourced file early-exits with status N (bash-faithful). Recursive depth capped at 64 to prevent runaway loops. `.` and `source` are aliases — both are added to `is_special_builtin`. Multi-line constructs are accumulated via `crate::continuation::classify`, matching the REPL's line-continuation behavior.
- **M-67: `local`** — `[fixed v52]` medium. Bash's `local` builtin for function-scoped variables. `local NAME=value` and `local NAME` (sets to empty) supported; multiple per call. On function return, each local is restored to its caller-side state (or unset if it was unset before). Outside a function → "can only be used in a function" + status 1. Idempotent within a single frame: `local X=1; local X=2` preserves the outer-caller's pre-`local` snapshot. Nested function calls have isolated local snapshots. Attribute flags (`local -p`/`-i`/`-a`/`-A`) deferred.
- **M-68: `:` (null command)** — `[fixed v53]` low. POSIX special builtin. Always exits 0 after huck's normal argument expansion runs, so `: ${VAR:=default}` triggers the param-expansion default-assignment side effect and `while :` is an infinite loop. Added to `is_special_builtin`.
- **M-69: `true` / `false`** — `[fixed v53]` low. Regular builtins. `true` exits 0; `false` exits 1. Both ignore their args (matches bash).
- **M-70: `command -v` / `-V`** — `[fixed v53]` medium. POSIX `command` builtin with `-v` (concise) and `-V` (verbose) introspection flags. Walks alias → function → builtin → keyword → `$PATH` (literal path if the name contains `/`). `-v` prints the name (or absolute path); `-V` prints "NAME is a shell builtin" / "NAME is a function" / "NAME is a shell keyword" / "NAME is aliased to \`value'" / "NAME is /path/to/NAME". Concise alias output runs the value through `escape_alias_value` so single-quotes are POSIX-escaped (`'\''`). Exit 0 if all names resolved, 1 if any missing. `-V` writes "huck: command: NAME: not found" to stderr on miss; `-v` is silent. Bare `command cmd args` (bypass function/alias lookup) and `-p` (default-PATH search) are deferred.
- **M-71: `readonly`** — `[fixed v54]` medium. POSIX special builtin. `readonly NAME=value` sets and locks; `readonly NAME` locks an existing var (or creates empty + locks if unset); multiple per call. `readonly` and `readonly -p` list all readonly vars in POSIX `readonly NAME='value'` format (single-quote escaping via the existing `escape_alias_value` helper). Eight write paths enforce the readonly flag: top-level `NAME=value`, inline `NAME=v cmd` (aborts the command via the existing snapshot+restore cycle), for-loop iter var, `${var:=...}` default-assignment, `$((var=...))` arithmetic, `export NAME=value` (bare `export NAME` is exempt — matches bash), `local NAME[=value]` in a function, and `unset NAME`. Each violation prints "huck: <context>: NAME: readonly variable" and returns status 1; for-loop and inline assignment additionally abort the body/command. Attribute extensions deferred: `readonly -f` (function readonly), `readonly -a`/`-A` (huck has no arrays). **Known limitation**: internal-mechanism writes (cd updating PWD/OLDPWD, signal-state) bypass the check by design — `readonly PWD; cd /tmp` succeeds in huck where bash would error. Acceptable for now. A second known limitation: `$((X=5))` on readonly X correctly fires the readonly check and leaves X unchanged, but the surrounding simple command's exit status reflects the OUTER command (e.g., `echo` succeeds and overwrites `$?` back to 0) — propagating arith-eval errors into `last_status` would be a separate refactor.

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
- **Status**: fixed (2026-05-26) by v25 (partially) + v26 (fully)
- **Severity**: medium
- **huck (was)**: `cd /tmp | true` mutated the parent shell's cwd because builtin pipeline stages ran in-process in the parent. Additionally, single-stage pure-builtin pipelines run as background jobs (`echo hi &`, `cd /tmp &`) bypassed the fork entirely via a `pipeline_is_pure_builtin` shortcut in `run_background_sequence`, so `$!` was never updated and side effects still leaked to the parent.
- **bash**: every pipeline stage runs in a subshell; side effects are local.
- **Fix**: v25 rewrote `run_multi_stage` so every multi-stage pipeline forks a subshell per stage (spec/plan dated 2026-05-25; informally "I-04" in the v25 spec, canonical ID here is I-16 since I-04 was already taken). v26 completed the fix by removing the `pipeline_is_pure_builtin` shortcut in `run_background_sequence` — all backgrounded pipelines now go through the same fork machinery regardless of whether they are pure-builtin, so `$!` receives a real pid and parent state is no longer mutated.

---

## Tier 4: Low-impact / edge cases

- **L-01**: `~user` lookup capped at 16 KiB buffer. (Never hit in practice.)
- **L-02**: Glob sort order is byte-lexicographic, not `LC_COLLATE`-aware.
- **L-03**: Non-integer variable in `$((…))` errors instead of bash's "treat as recursive arith expression."
- **L-04**: `${#var}` counts Unicode chars; bash counts bytes (matches in UTF-8 locale). v33 extends the same char-counting convention to `${var:off:len}` — offset/length units are codepoints, never byte indices. Slices never split a multi-byte UTF-8 codepoint. v37 `${var^^}` / `${var,,}` uses Rust's `char::to_uppercase` / `char::to_lowercase` Unicode iterators — locale-independent (matches bash with UTF-8 locale; may differ in non-UTF-8 locales).
- **L-05**: `[N] PID` spawn notification shows only the last pipeline stage's PID; bash shows all.
- **L-06**: `jobs` column width is fixed at 24; bash uses terminal width.
- **L-07**: `wait` polls (50ms) rather than blocking — small latency / minor CPU usage.

### L-08: Redirect source-order not preserved (`2>&1 >file` anti-pattern)
- **Status**: intentional (v29)
- **Severity**: low
- **huck**: `cmd 2>&1 >file` is treated identically to `cmd >file 2>&1` — both fds end up at the file. The field-based `ExecCommand` AST (`stdin`/`stdout`/`stderr`) stores at most one redirect per fd and cannot preserve source order.
- **bash**: `cmd 2>&1 >file` puts stderr to the terminal and stdout to the file (because `2>&1` dups stderr to the CURRENT stdout, which is the terminal at that point, then `>file` redirects stdout). The canonical form is `cmd >file 2>&1`.
- **Why intentional**: source-order preservation requires refactoring `ExecCommand` to `redirects: Vec<(SourceFd, Redirect)>` — a substantial change. The canonical form covers >99% of real usage.
- **Workaround**: write `cmd >file 2>&1` (or `cmd &>file`).

### L-09: Regex `=~` is RE2-style, not POSIX ERE
- **Status**: intentional (v30)
- **Severity**: low
- **huck**: `[[ $s =~ regex ]]` uses the Rust `regex` crate (RE2-based). No lookbehind / lookahead (`(?<=...)`, `(?=...)`); minor syntax differences from POSIX ERE for some edge cases (e.g., `(?:...)` non-capturing groups are supported in both, but bash's POSIX-mode is stricter).
- **bash**: POSIX ERE. Has its own quirks.
- **Why intentional**: `regex` is a mature, fast, well-maintained Rust crate. Implementing POSIX-ERE-faithful regex isn't worth the cost for the rare divergences. Most real-world shell-regex usage works identically.
- **Workaround**: if a script relies on POSIX-ERE-specific features, fall back to `grep -E "pattern"` (which uses libc's POSIX ERE).

### L-10: `${var:…}` and `${var/…/…}` mis-split on `:` or `/` inside command substitutions
- **Status**: intentional (v33)
- **Severity**: low
- **huck**: `${s:$(echo 1:2)}` corrupts the split — the inner `:` inside `$(echo 1:2)` is treated as the offset/length delimiter, yielding offset `$(echo 1` and length `2)`. Same issue for `${var/$(cmd/with/slash)/repl}`. The `scan_braced_operand` helper handles brace depth and quoted spans but does not depth-track `$(…)` or `$((…))`, so the second-pass split scanners (`split_substring_body`, `split_substitution_body`) see the inner metacharacter at depth 0.
- **bash**: parses at the grammar level so the inner metacharacter is never visible to the split.
- **Why intentional**: real scripts almost never put a `:` or `/` literal inside a command substitution that itself sits inside a parameter expansion operand. The split scanners are simple by design, and adding `$(`/`$((` depth tracking would touch both v32 and v33 helpers for a vanishingly rare pattern.
- **Workaround**: stash the command-substitution result in a variable and reference the variable inside the operand.

### L-11: `$'\xHH'` and `$'\nnn'` produce Unicode codepoints, not raw bytes

Bash inserts the raw byte value (0x00–0xFF) directly into the output
string. huck, whose strings are Rust `String` (UTF-8), interprets the
numeric value as a Unicode codepoint via `char::from_u32`. For ASCII-range
values (< 0x80) the two encodings are bit-identical. For high-bit values
the divergence is visible: bash's `$'\xFF'` is a single byte (`0xFF`),
huck's `$'\xFF'` is the two-byte UTF-8 encoding of U+00FF
(`0xC3 0xBF`).

This aligns with L-04 (huck's Unicode-by-default convention for parameter
expansion). Scripts that depend on injecting raw high bytes via
`$'\xHH'` — rare in practice — will see different output sizes.
Surrogate-range escapes (`\uD800`..`\uDFFF`) and codepoints above
U+10FFFF are rejected with a `LexError::AnsiCInvalidCodepoint` rather
than silently producing invalid UTF-8.

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
- **2026-05-26**: I-16 fix completed by v26 post-review patch. v25's fix covered multi-stage pipelines but a `pipeline_is_pure_builtin` shortcut in `run_background_sequence` still ran single-stage pure-builtin backgrounds synchronously in the parent (preventing `$!` update and leaking side effects). Shortcut removed; all backgrounded pipelines now fork. M-10 fixed-date corrected to 2026-05-25.
- **2026-05-26**: M-13 (here-strings `<<<word`) shipped as v27.
- **2026-05-26**: M-11 (subshell syntax `(list)`) shipped as v28.
- **2026-05-26**: M-18/19 (fd-duplication redirects) shipped as v29. Supports `2>&1`, `1>&2`, `&>file`, `&>>file`. Documented order-divergence for `2>&1 >file` anti-pattern as L-08.
- **2026-05-26**: v30 Task 3 follow-up: added `dbracket_in_while` integration test (was in spec table, missed by implementer); added `parse_dbracket_with_inline_assignment` parser unit test strengthening the speculative-peel path. Logged B-11 (`false; echo $?` prints 0 instead of 1 — pre-existing bug, entire test suite works around it via `\n` separators).
- **2026-05-26**: M-14 (`[[ ]]` extended test) shipped as v30. Regex engine is `regex` crate (RE2-style; L-09 documents the divergence from POSIX ERE).
- **2026-05-26**: B-11 fixed (v31). `execute_sequence_body` now calls `shell.set_last_status` after each command's `Continue(c)` outcome so `$?` propagates across `;`/`&&`/`||` within a sequence. Tier 1 is empty again.
- **2026-05-26**: M-15 (`${var/pat/repl}` pattern substitution) shipped as v32. All six bash forms: first-match `/`, all-matches `//`, anchored-prefix `/#`, anchored-suffix `/%`, plus empty-replacement shortcut. New `ParamModifier::Substitute` + `SubstAnchor` AST, new lexer `Some('/')` arm with `scan_substitution_operand` (`\/` escapes literal slash), `substitute()` evaluator using `glob::Pattern` over a `char_indices` boundary scan. Empty pattern is a no-op (bash-compat); trailing empty match suppressed so `${var//*/Q}` emits a single replacement.
- **2026-05-27**: M-16 (`${var:off:len}` substring expansion) shipped as v33. Scalar vars + positional params; full arith in offset/length via reuse of v22's `arith::parse` + `arith::eval`; bash 5.x edge-case semantics with char-counting per L-04. Inherits M-58 divergence (PE errors don't abort the surrounding command). Array slicing on `$@`/`$*` deferred.
- **2026-05-27**: M-58 (fatal PE error abort) + M-60 (length-of-positional / count-of-positionals) shipped as v34. New `ExpansionResult::Fatal` variant carries the abort signal from `expand_modifier`; the three `expand_*` functions stash it on `Shell::pending_fatal_pe_error`; the executor's `resolve()` + `execute_sequence_body` peek-check it; the REPL drains and (in non-interactive mode) exits the shell. M-16's M-58-inheritance note removed.
- **2026-05-27**: M-22 (`trap` builtin) shipped partially as v35. EXIT pseudo-signal + 13 trappable real signals via huck's existing 15-name table. New `src/traps.rs` module owns signal-handler installation (via `signal_hook::low_level::register`), the `Arc<AtomicU32>` pending-signal bitmask, and the dispatch/fire helpers. ERR/DEBUG/RETURN pseudo-signals deferred to a follow-up iteration; M-22 stays `[partial v35]` until they land.
- **2026-05-27**: M-22 (`trap` builtin) closed as fixed v36. ERR, DEBUG, RETURN pseudo-signals added alongside v35's EXIT + 13 real signals. Per-event firing helpers (`fire_err_trap` / `fire_debug_trap` / `fire_return_trap`) share a `fire_pseudo_trap` body with a `Shell::firing_trap` recursion guard. ERR uses a `Shell::err_suppressed_depth` counter pushed/popped in `run_if` / `run_while` to implement bash 5.x exemptions. DEBUG hooks at `run_exec_single` entry; RETURN at `call_function` with $? set to the function's status. M-22 status: `[fixed v36]`.
- **2026-05-28**: M-17 (`${var^^}` / `${var,,}` case modification) shipped as v37. All eight forms (`^^`/`^`/`,,`/`,` × bare/with-pattern). Reuses glob::Pattern for the per-character pattern filter; Rust's `char::to_uppercase` / `char::to_lowercase` iterators for the Unicode-aware case mapping. Closes the parameter-expansion-modifier cluster started by v32/v33/v34.
- **2026-05-28**: M-55 (bitwise operators), M-56 (assignment + inc/dec), and M-57 (non-decimal literals) shipped together as v38 — closes the arithmetic-feature cluster started by v22. Bundled `**` exponentiation. `arith::eval` signature changed from `&Shell` to `&mut Shell` (3 call sites updated). Pratt-parser precedence table renumbered to match bash's documented order. Shift counts out of `[0, 64)` produce explicit errors (deliberate divergence from bash's C-undefined behavior).
- **2026-05-28**: M-28 (`$'…'` ANSI-C quoting) shipped as v39. New arm in `read_dollar_expansion` dispatches to `read_ansi_c_quoted` + `decode_ansi_c_escape`. All 16 bash escape forms supported. Numeric escapes resolve to Unicode codepoints (new L-11 divergence for `\xHH`/`\nnn` > 0x7F). Unknown escapes preserve `\` + following char. New `LexError::AnsiCInvalidCodepoint(u32)` for surrogates / out-of-range values. Pure lexer change — no parser, AST, executor, or expansion changes.
- **2026-05-28**: M-37 (`wait -n` with optional `-p VAR`) and M-38 (multi-arg `wait`) shipped together as v40. `builtin_wait` rewritten as a flag/positional parser (`parse_wait_args`) feeding a 5-way dispatch over `(wait_any, targets.len())`. Three new helpers (`wait_for_all`, `wait_any_pending`, `wait_any_of`) reuse the existing `waitpid(-1, WNOHANG)`-poll machinery. `-p VAR` captures the finished job's pgid (for `%spec` targets) or literal PID (for PID targets). All changes confined to `src/builtins.rs`. No new L-* divergences.
- **2026-05-28**: M-39 (`kill -l` with all bash forms) shipped as v41. New `killable_signals()` table in `src/traps.rs` (14 trappable + KILL + STOP). `builtin_kill` extended with `-l` short-circuit + new `handle_kill_l` / `print_killable_table` helpers; `signal_by_name` deduplicated to share the same table (also fixes `kill -WINCH pid`). README "Not yet implemented" paragraph trimmed to remove items shipped in v33, v37, v40, and v41. No new L-* divergences.
- **2026-05-28**: M-40 (`kill -s SIGNAME` + `kill -n SIGNUM`) shipped as v42. Extracted the existing per-target send loop into a shared `send_signal_to_targets` helper; added `kill_with_s_flag` and `kill_with_n_flag` dispatch arms. Reuses v41's `signal_by_name` (SIG-prefix + case-insensitive) and `killable_signals()` (16-entry table). Usage string bumped. No new L-* divergences.
- **2026-05-28**: M-43 (`disown -a`/`-r`/`-h` + multi-arg) shipped as v43. New `Job.marked_for_nohup` field + `JobTable::mark_for_nohup` helper + `Shell::hangup_jobs` method + pure `should_hangup` predicate. `builtin_disown` rewritten as a flag parser + multi-arg dispatcher with combined-flag support (`-ah`, `-ar`, `-arh`). The five clean-exit sites in `src/shell.rs::run` now call `shell.hangup_jobs()` before `history.save()`. **Behavior change**: bg jobs now receive SIGHUP on clean shell exit (was: never sent); scripts relying on huck's old "always survives" behavior need to add `disown -h`. No new L-* divergences.
- **2026-05-29**: M-44 (`disown` accepts bare PID) shipped as v44. One-arm change in `builtin_disown`'s positional loop: non-`%` args now parse as positive `i32` and look up the matching job via `shell.jobs.iter().find(|j| j.pids.contains(&pid))`. Match scope includes non-leader pipeline stages; the whole job is operated on. No new L-* divergences.
- **2026-05-29**: M-45 (`jobs` flag filters + positional `%spec`) shipped as v45. New `notification_line_long` formatter in `src/jobs.rs` for bash-faithful multi-line `-l` output; new `JobTable::mark_notified` helper consumed by `-n`. `builtin_jobs` rewritten as a flag parser + filter dispatcher with three output modes (default, `-l`, `-p`). All five bash filters supported with combined forms; `-r`/`-s` are AND-combined (mutually exclusive in practice); `-p` overrides `-l` per bash. Positional `%spec` args resolve via the existing `resolve_spec_or_error`. No new L-* divergences.
- **2026-05-29**: M-61 (brace expansion) shipped as v46. New `src/brace_expand.rs` module with recursive `expand` algorithm covering comma lists, integer ranges (asc/desc/step/zero-pad), char ranges, prefix/suffix, nested, and Cartesian product. Lexer integration in `src/lexer.rs` routes every Word emission through `emit_word_with_braces`; sentinel-bearing concat using Unicode Private Use Area chars (`\u{E000}`/`\u{E001}`) preserves Var/Arith/CommandSub/Tilde and quoted Literals across expansion. New `LexError::BraceExpansionLimit` variant for the 65,536 safety cap. No new L-* divergences.
- **2026-05-29**: M-62 (extended job specs `%cmd` / `%?cmd`) shipped as v47. `JobSpec` enum extended with `Prefix(String)` and `Substring(String)` variants; `parse_job_spec` recognizes the new forms (the previously-error `%abc` becomes a valid `Prefix`). New `JobSpecResolveError { NotFound, Ambiguous }` enum; `JobTable::resolve` signature changed from `Option<u32>` to `Result<u32, JobSpecResolveError>`. Single call site (`resolve_spec_or_error`) updated to surface the Ambiguous arm as `huck: <builtin>: <arg>: ambiguous job spec` + status 1. No new L-* divergences.
- **2026-05-29**: M-63 (aliases) shipped as v48. New `src/alias_expand.rs` module with `expand_aliases_in_tokens` algorithm — tracks command position, recursively substitutes via per-input cycle-protected `active` HashSet, supports the bash trailing-space rule. New `Shell.aliases: HashMap<String, String>` field. New `alias` and `unalias` builtins. `process_line` gained an `expand_aliases: bool` parameter so the REPL can pass `shell.is_interactive` while trap firings pass `false`. Added `HUCK_EXPAND_ALIASES` env-var override to enable expansion under non-interactive stdin (e.g. piped scripts, integration tests). No new L-* divergences.
- **2026-05-29**: M-52 (backgrounded multi-pipeline sequences) shipped as v49. Two-file change. Parser unblocked: removed `!rest.is_empty()` rejection and the now-unused `ParseError::BackgroundedMultiPipelineSequence` variant. Executor extended: `execute()` gains a new arm for `seq.background && !seq.rest.is_empty()` that synthesizes `Command::Subshell { body: Box::new(<seq>) }` and dispatches to the existing `run_background_subshell`. Reuses fork + JobTable + `[N] PID` notice infrastructure. No new L-* divergences.
- **2026-05-29**: M-65 (`shift` + `set --`) shipped as v50. Single-file change in `src/builtins.rs`. `builtin_shift` mutates `Shell.positional_args` via `drain(0..n)` with bounds + numeric validation. `builtin_set` lists vars (no args) or replaces positional via `args[1..]` after `--` or via `args[..]` for the bare form. Option flags rejected with status 2. Both added to `BUILTIN_NAMES`, `run_builtin` dispatch, and `is_special_builtin`'s matched set. The `is_special_builtin` doc comment is trimmed to drop set/shift from the future-additions list. No new L-* divergences.
- **2026-05-29**: M-66 (`source` / `.`) shipped as v51. New `Shell.source_depth: u32` field. `builtin_source` resolves the path (literal if slash present, else `$PATH` lookup), reads the file, optionally pushes extra args as positional, increments depth, and runs lines with `crate::continuation::classify` handling multi-line accumulation. `return` at the source's top level catches as `Continue(n)` (bash-faithful early-exit); `exit` propagates as-is. `lex_error_message` and `parse_error_message` in `src/shell.rs` gained `pub(crate)` visibility so the source builtin can render parse errors in the REPL's format. No new L-* divergences.
- **2026-05-29**: M-67 (`local`) shipped as v52. New `Shell.local_scopes: Vec<HashMap<String, Option<Variable>>>` stack. `call_function` pushes an empty frame before running the body; after the body (and the RETURN trap), pops the frame and replays each saved snapshot via the new `restore_var` method. `Variable` becomes pub so it can be referenced in the field type. `builtin_local` errors if `local_scopes` is empty, else parses `NAME=value` / `NAME`, snapshots pre-state once per name via the new `snapshot_var` method, and `shell.set(name, value)`s. Added to `BUILTIN_NAMES` and `run_builtin` dispatch; NOT added to `is_special_builtin` (bash classifies as regular). No new L-* divergences.
- **2026-05-29**: M-68 (`:`), M-69 (`true`/`false`), M-70 (`command -v`/`-V`) shipped together as v53 — the trivials cluster. Four small builtins in `src/builtins.rs`: `builtin_colon` and `builtin_true` return `Continue(0)`, `builtin_false` returns `Continue(1)`, all ignoring args. `builtin_command` parses `-v`/`-V` flags from the left and resolves each remaining name via `resolve_command_name` (alias → function → builtin → keyword → `$PATH`). Helpers: `CommandResolution` enum, `is_shell_keyword` (hardcoded set), `search_path_for` (handles names containing `/` as literal paths; otherwise iterates `$PATH` split on `:`, skipping empty segments), `is_executable_file` (Unix `mode & 0o111`). Concise alias output reuses the existing `escape_alias_value` helper so output is shell-parseable. `:` added to `is_special_builtin`; `true`/`false`/`command` regular. Bare-form `command cmd args` rejected with status 2 (deferred). 13 unit tests (added regression test for alias single-quote escaping) + 8 integration tests. Also updates `tests/wait_integration.rs::wait_multiarg_all_succeed` to use sleeps — making `true` a builtin removed the implicit timing buffer that test relied on. No new L-* divergences.
- **2026-05-29**: M-71 (`readonly`) shipped as v54. `Variable` gains a `pub readonly: bool` field; all existing literals defaulted to false. New Shell methods: `is_readonly`, `try_set`, `try_unset`, `mark_readonly`, `readonly_names`. `builtin_readonly` (POSIX special; added to `is_special_builtin` and `BUILTIN_NAMES`) parses `-p` and `--`; with no names lists; with names sets value (if `=`) and marks readonly; invalid identifiers → status 1; overwriting already-readonly → status 1. Enforcement plumbed into 8 write paths: `builtin_unset`, `builtin_export` (value form only — bare `export NAME` exempt), `builtin_local` (both forms), top-level `SimpleCommand::Assign`, `apply_inline_assignments` (signature now `Result<Snapshot, Snapshot>`; FOUR call sites updated — `run_double_bracket`, inner pipeline path, `run_exec_single`, main `run_multi_stage`), for-loop iter, `${var:=…}` (`ExpansionResult::Fatal { status: 1 }`), and arithmetic assignment (new `ArithError::ReadonlyVar` variant; `write_var_i64` now returns `Result<(), ArithError>`). Code-quality review during Task 1 caught `Shell::export_set` silently stripping readonly when overwriting an existing entry (no user-facing path exposes this currently — bare `export` goes through `shell.export` which preserves flags; `export NAME=v` is guarded by `is_readonly` — but Task 2's `apply_inline_assignments` would have been the first reachable caller); fix mirrors `set`'s update-in-place pattern. 17 unit tests (11 builtin including regression for export_set-preserves-readonly + 6 executor) + 6 integration tests. No new L-* divergences.
