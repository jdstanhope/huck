# bash 5.2.21 test-suite baseline

bash source: 5.2.21 (GNU, GPLv3+; not vendored, run from `$BASH_SOURCE_DIR`).
huck commit: 8b75908.
Sweep date: 2026-06-24 UTC.

## Summary

- Categories run: 82
- PASS: 5
- FAIL: 73
- TIMEOUT: 4
- ERROR: 0
- SKIP (from known-skips.txt): 4

## Per-category status

| Category | Status | Note |
|---|---|---|
| alias | FAIL | Error-message format divergence — huck uses its own name as the command-not-found prefix rather than the running script's filename; also some alias-expansion differences in non-interactive script mode. |
| appendop | FAIL | Multiple gaps: an array-element append subscript form that huck fails to parse; assoc-array iteration-order divergence (L-44); and readonly-assignment abort difference (L-43). |
| arith | FAIL | The `set -o posix` / `set +o posix` cascade is resolved in v215; the test suite now runs end-to-end. Remaining failures are genuine arithmetic divergences: error-message format differs throughout (huck omits the source-file and line-number context and uses different category wording, while bash includes the originating line reference and an error-token excerpt); signed-integer overflow is rejected by huck as out-of-range while bash wraps silently to the minimum signed value; increment/decrement on non-lvalue literals, assignment in short-circuit arithmetic contexts, and ternary-expression error cases each produce mismatched output. |
| arith-for | FAIL | `declare -f` trailing-space format divergence remains (bash appends a trailing space after the function-name-and-parens and opening-brace tokens; huck omits it). Functions whose bodies contain arith-for constructs with non-standard headers may fail to be defined in huck, causing `declare -f` to emit nothing where bash outputs the full reformatted body. Error-message text for intentionally invalid `for ((` headers (wrong section count or a quoted string as a section value) differs between huck and bash. |
| array | FAIL | `set +a` (all-export off) not supported, misconfiguring the test environment. Also an array literal whose element contains a background `&` operator is parsed differently than bash expects. |
| array2 | FAIL | Test infrastructure: requires `recho` helper binary compiled from bash source. That binary is not present, so most test cases fail before reaching the array operations themselves. |
| assoc | FAIL | `BASH_ALIASES` and `BASH_CMDS` built-in assoc arrays are not present in huck. Also L-46 (bare attribute-only `declare -A` prints an empty-string assignment in `declare -p`) and L-44 (assoc-array iteration order). |
| attr | FAIL | `readonly -a` (array readonly flag) not recognized — huck rejects the `-a` option. Error-message prefix format differs throughout. New bug. |
| braces | FAIL | L-38 (brace expansion ordering when a brace follows a parameter or appears in a scalar-assignment RHS). Also character-range brace expansion differences with backslash characters, and test infrastructure (`recho`/`zecho` not found). |
| builtins | FAIL | Multiple unimplemented `set -o` options (`posix`, `+p`) abort the test preamble. `ulimit` and `fc` are not found as commands. |
| case | FAIL | L-43 (readonly variable assignment does not abort a non-interactive shell) — bash exits the script on the first readonly violation in the case tests while huck continues, cascading through all subsequent case-statement comparisons. |
| casemod | FAIL | Case-modification operations on multi-word arrays produce output in a different word order than bash expects — likely L-44 (array/assoc iteration order) affecting the loop variable sequence. |
| complete | FAIL | M-92 (`${!prefix@}` variable-name-listing expansion) not implemented — `complete.tests` uses this inside a `[[ ]]` expression, causing an unterminated-compound-test parse error that prevents the entire suite from running. |
| comsub | FAIL | Test infrastructure: `recho` not found. Additional command-substitution edge cases likely diverge further. |
| comsub-eof | FAIL | Unterminated heredoc inside a command substitution is treated as a hard error in huck (aborts the substitution) while bash issues a warning and treats the EOF as the delimiter. New divergence in error-vs-warning handling. |
| comsub-posix | FAIL | Same unterminated-heredoc-in-command-sub divergence as comsub-eof, plus additional failures from missing posix-mode set option. |
| cond | FAIL | M-94 (`${!@}` / `${!*}` as indirect expansion of the positional list) causes a parse error, aborting the test early. Additional `[[ ]]` compound-test edge case with an incomplete expression. |
| coproc | FAIL | Coproc pipe file-descriptor numbers diverge — huck allocates low-numbered fds while bash allocates high-numbered fds. Also `<&N-` / `>&N-` dup-and-close fd redirect operator not supported (same root cause as the redir TIMEOUT). M-126 and a new dup-close gap. |
| cprint | FAIL | `declare -f` trailing-space format divergence — bash appends a trailing space after the function-name and opening-brace tokens; huck omits it. Same class as arith-for/func/herestr. |
| dbg-support | FAIL | `set -o functrace` (DEBUG/RETURN/ERR trap inheritance through function calls) not yet supported. Entire debug-trap test suite fails from the first rejected option. |
| dbg-support2 | FAIL | `LINENO` inside functions reports the logical-command start line (often line 1) rather than the actual in-function source line. New bug: LINENO tracking accuracy inside function bodies. |
| dirstack | FAIL | `pushd -m` / `popd -m` / `dirs -m` argument is treated as an invalid option rather than a numeric argument (huck and bash differ on which flags these commands accept). Error-message prefix and format differences throughout. |
| dollars | FAIL | Test infrastructure (`recho` not found). Also M-94 (`${!*}` parsing error) causes early test abort. |
| dynvar | FAIL | `BASH_ARGV0` is not updated to reflect the running script's `$0` — tests that check `BASH_ARGV0` report a mismatch. `EPOCHREALTIME` not implemented (L-41 computed-dynamics gap). |
| errors | FAIL | Multiple `set -o <option>: not yet supported` rejections misconfigure the test environment (posix, allexport, etc.). Also `alias -x` / `unalias -x` flags not recognized. Cascading from missing set options. |
| execscript | FAIL | Error-message format differences — huck uses its own name as prefix rather than the script-file-and-line-number form bash uses. Executing a binary file produces a UTF-8 decoding error instead of bash's "cannot execute binary file" message. |
| exp-tests | FAIL | Test infrastructure (`recho` not found). Exercises many parameter-expansion edge cases that likely have additional divergences. |
| exportfunc | FAIL | M-09a (relaxed function-name characters) — function names containing hyphens (e.g., `foo-a`) are rejected by huck's identifier parser. Additional divergences in heredoc-count limits and export-flag error handling. |
| extglob | FAIL | A subset of extglob patterns involving backslash-escaped metacharacters inside extglob brackets diverge from bash. Also test infrastructure (`recho` not found, temp-directory permission issue). Core extglob matching is mostly correct; edge cases remain. |
| extglob2 | PASS | |
| extglob3 | PASS | |
| func | FAIL | `declare -f` trailing-space format divergence (same class as cprint/arith-for). A few function-body variable-capture edge cases diverge. |
| getopts | FAIL | Usage-message format divergence — huck omits the trailing ellipsis from the optional-argument notation and uses its own name as message prefix. Pre-existing L-26 class divergence. |
| glob-test | FAIL | Test infrastructure: `recho` binary not found; also emits a locale-unavailability warning. L-04/L-11 (character vs byte in multibyte glob expansion) causes additional divergences on high-byte characters. |
| globstar | FAIL | Test environment mismatch — `globstar.tests` expects to run from the bash build directory (where compiled object files are present to glob over); huck runs it from the tests directory, where those files do not exist. Also M-53 (bare `**` globstar matches directories only, not files). |
| heredoc | FAIL | Several heredoc edge cases: a `$PS4` literal appears in huck's heredoc output where bash expects an expanded (or empty) value; fd-based heredoc reads via an `exec`-opened descriptor generate bad-fd errors; and an unterminated heredoc inside a complex script aborts where bash would continue. |
| herestr | FAIL | `declare -f` reconstruction of here-string expressions — adjacent quoted-string concatenation is collapsed and quoting style changes. Same trailing-space format class as cprint/func/arith-for. |
| histexpand | FAIL | `set: history: not yet supported`, and history-expansion flags (`-p`, `-a`, `-s`, `-w`) not implemented (M-46). Entire test suite fails from the first rejected option. |
| history | FAIL | M-46 (`history -d/-w/-r/-a` not supported), M-47 (`history N` numeric argument not supported), `fc` not found as a command, `set: history: not yet supported`. Multiple history-command gaps. |
| ifs | FAIL | `set: posix: not yet supported` misconfigures the test environment. Also test infrastructure (`recho` not found). Some IFS-splitting edge cases may diverge independently. |
| ifs-posix | FAIL | IFS splitting semantics with the `read` builtin diverge when IFS contains both whitespace and non-whitespace characters — huck does not correctly handle certain adjacent mixed-class IFS-separator edge cases. New bug, separate from the unimplemented posix set option. |
| input-test | FAIL | A line piped to a sub-script via a process pipeline is not read correctly — the sub-script's `read` sees an empty value instead of the piped content. New bug in how huck passes piped input to a child script invocation. |
| invert | PASS | |
| iquote | FAIL | Test infrastructure (`recho` not found). Exercises in-quote expansion edge cases. |
| jobs | TIMEOUT | Tests include deliberate multi-second `sleep` waits for process-synchronization; the accumulated sleep time exceeds the 30-second per-category timeout. Job-control behavior not fully assessed due to timeout. |
| lastpipe | FAIL | `shopt -s lastpipe` not implemented — with lastpipe enabled bash runs the final pipeline stage in the current shell so its assignments are visible after the pipe. Huck always forks all pipeline stages; variables set in the last stage are not visible. New missing feature. |
| mapfile | FAIL | L-34 (`mapfile -C` callback and `mapfile -u` fd-argument flags not implemented). Documented deferred gap from v140. |
| minimal | TIMEOUT | Compound runner that includes `run-read` (which hangs on `read -t` blocking indefinitely); when that sub-test hangs, the entire minimal suite times out. |
| more-exp | FAIL | Test infrastructure (`recho` not found). Exercises many expansion edge cases. |
| nameref | FAIL | L-47 (nameref follow-on gaps). A `declare -p` call on a nameref variable dumps the entire variable table instead of just the named variable — new bug in the nameref plus `declare -p` interaction path. |
| new-exp | FAIL | `set: posix: not yet supported` misconfigures the test environment; also test infrastructure (`recho` not found) and arithmetic parsing edge cases. |
| nquote | FAIL | Test infrastructure (`recho` not found). Exercises quoting edge cases. |
| nquote1 | FAIL | Test infrastructure (`recho` not found). Also exercises array operations that likely have additional divergences. |
| nquote2 | FAIL | Test infrastructure (`recho` not found). Also exercises array operations. |
| nquote3 | FAIL | Test infrastructure (`recho` not found). Also exercises array operations. |
| nquote4 | FAIL | Test infrastructure (`recho` not found). Some tests require UTF-8 locale. |
| nquote5 | FAIL | Test infrastructure (`recho` not found). |
| parser | FAIL | Error-message format divergence — huck uses its own name as prefix rather than the invoking-script-and-line form bash uses. Same L-class format divergence as execscript/type/dirstack. |
| posix2 | FAIL | Three POSIX compliance failures: OPTIND initial value, variable-quoting edge cases, and the `case esac` pattern (using the keyword `esac` as a pattern value, which is L-20 class — case pattern inside a complex context). |
| posixexp | FAIL | Test infrastructure (`recho` not found). Exercises POSIX parameter-expansion cases. |
| posixexp2 | FAIL | `set: posix: not yet supported` misconfigures the test environment; also an unterminated `${...}` handling difference when posix mode is presumed active. |
| posixpat | FAIL | POSIX bracket-expression edge cases — specifically certain character-range and negated-bracket patterns where huck and bash produce different match results. New bug in the POSIX-ERE-adjacent glob matching path. |
| posixpipe | FAIL | `time` builtin output format differs (huck emits the system `time(1)` format while bash uses its own built-in format with `real`/`user`/`sys` labels). Also lastpipe behavior divergence. |
| precedence | PASS | |
| printf | FAIL | Usage-message prefix format (`huck: printf: usage:` vs bare `printf: usage:`). Also some format-specifier differences (string width and `%b` handling). |
| procsub | FAIL | L-39 (process-substitution edge cases) — the FIFO fallback path produces permission-denied errors when the test environment lacks writable `/dev/fd`. Core `<(cmd)` on standard fds also fails with a permission error in this run. |
| quote | FAIL | Backslash quoting edge cases — an escaped space inside a word is treated differently, and a backslash-newline line continuation produces two separate values rather than joining the words. New bugs in backslash-quote-in-word handling. |
| quotearray | FAIL | Assoc-array keys containing escaped special characters (brackets, dollar signs, backslashes) cannot be used as arithmetic subscripts — the arith parser fails on the key content. New bug in special-character key handling in arithmetic array contexts. |
| read | TIMEOUT | `read -t` (timeout option) not implemented (L-34) — tests that issue `read -t N` with a pipeline or tty source block indefinitely instead of timing out. `read -u` (fd argument) also not implemented (L-34). |
| redir | TIMEOUT | The `<&N-` and `>&N-` dup-and-close fd redirect operators are not supported — huck rejects the close modifier, leaves fd state in an inconsistent condition, and a subsequent unconditional `read` blocks on terminal input indefinitely. New bug in dup-and-close redirect syntax. |
| rhs-exp | FAIL | Test infrastructure (`recho` not found). Exercises right-hand-side expansion. |
| set-e | FAIL | `set -e` interaction with `&&`/`||` compound lists, `!` negation, and `eval` diverges — some cases where bash would abort the script huck continues (or vice versa). New bug area in `set -e` compound-list abort semantics. |
| set-x | FAIL | Minor xtrace format difference: `(( expr ))` trace emits no trailing space in huck but bash includes one. Pre-existing L-21 residual. |
| shopt | FAIL | Error-message prefix format difference. Many `set -o <option>: not yet supported` rejections (allexport, braceexpand, hashall, histexpand, keyword, monitor, notify, onecmd, privileged, history, ignoreeof, interactive-comments, posix, emacs, vi). Significant missing set-option surface. |
| strip | PASS | |
| test | FAIL | `test <` and `test >` lexicographic string-comparison operators not supported — huck rejects them with "unexpected argument". Also `/dev/tty` inaccessible in the test runner environment (test infrastructure). |
| tilde | FAIL | `set: posix: not yet supported` misconfigures the test environment. Also tilde expansion inside colon-delimited variable-assignment values diverges in some posix-mode edge cases. |
| tilde2 | FAIL | Test infrastructure (`recho` not found). Also tilde expansion in PATH manipulation edge cases and `set: posix: not yet supported`. |
| trap | FAIL | `trap -p` display format divergence — huck prints bare signal names (`HUP`, `INT`, etc.) while bash prints them with the `SIG` prefix (`SIGHUP`, `SIGINT`, etc.). Subshell EXIT trap not firing when expected. Signal-number display differences in job-notification lines. Multiple trap formatting gaps. |
| type | FAIL | Error-message prefix format difference (L-class), `set: posix: not yet supported`, and `declare -f` output format issues cascade into function-display comparisons. |
| varenv | FAIL | Test infrastructure (`recho` not found). Also `set: -k` (keyword mode) not supported. |
| vredir | FAIL | Variable fd redirection (`exec {varname}>file`) not implemented — huck does not support the `{varname}` syntax that allocates a fresh file descriptor and assigns its number to the named variable. New missing feature. |

## Skipped categories

| Category | Reason |
|---|---|
| loadable | huck has no loadable-builtin support; bash-specific. |
| intl | depends on locale/i18n infrastructure; out of huck's compat scope. |
| strict-posix | depends on POSIX-strict mode huck doesn't implement. |
| rsh | restricted shell (`set -r`) is not implemented and is not a huck feature. |

## How to regenerate

1. `curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp`
2. `export BASH_SOURCE_DIR=/tmp/bash-5.2.21`
3. `bash tests/bash-test-suite/runner.sh > /tmp/sweep.md`
4. Hand-triage non-PASS categories using the per-category diffs printed
   in the runner's header path.
5. Update this document with the new status column and prose Notes.
6. Commit.

## Licensing reminder

This document contains only huck-authored content (category names,
status counts, prose notes). NEVER copy verbatim bash test output or
test-script contents into the Note column — those bytes are GPL'd.
The full per-category diffs live in `/tmp/huck-bash-tests-<timestamp>/`
and stay local.
