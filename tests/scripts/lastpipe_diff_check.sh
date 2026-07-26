#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v338 `shopt -s lastpipe` (#306).
#
# Under `lastpipe` (+ job control off + a Terminal-sink foreground pipeline),
# bash runs the LAST stage of a multi-stage pipeline in the current shell
# instead of forking it: its variable assignments persist, its exit status /
# control flow (exit/return/break/continue) apply to the shell, and it is
# still recorded in $PIPESTATUS.
#
# Each fragment turns lastpipe on with `shopt -s lastpipe` and runs at the
# top level (a Terminal-sink, non-interactive script — matching the gate).
#
# NOTE (dropped fragment): `shopt -s lastpipe; echo "cap:$(echo a b c | read
# z; echo "[$z]")"` diverges — inside `$()` bash still runs the pipeline in
# the subshell created for the command substitution, so `read z` persists
# WITHIN that subshell and bash prints `cap:[a b c]`. huck's lastpipe gate
# requires `StdoutSink::Terminal`, which a capture context is not, so huck
# forks the last stage as before and prints `cap:[]`. Capture-context
# lastpipe is an intentional follow-up (not filed as of this iteration) —
# see docs/superpowers/plans/2026-07-26-lastpipe.md Task 2. Verified against
# real bash 5.2.21 before locking in every fragment below.
set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s\n' "$label"
        PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        diff <(echo "$b") <(echo "$h") | sed 's/^/    /'
        FAIL=$((FAIL+1))
    fi
}

# ---- variable persistence -------------------------------------------------

# 1. `read` as the last stage: $foo persists after the pipeline (bash: "a b c").
check "persist read" \
      'shopt -s lastpipe; echo a b c | read foo; echo "foo=$foo"'

# 2. `while read` loop as the last stage: the loop's own assignments persist.
check "persist while accumulate" \
      'shopt -s lastpipe; tot=0; printf "%d\n" 1 2 3 | while read n; do tot=$((tot+n)); done; echo $tot'

# 3. An assign-only last stage persists too (not just a builtin/compound).
check "persist assign-only" \
      'shopt -s lastpipe; unset bar; echo g h i | bar=7; echo "bar=$bar"'

# 4. A `while read` loop's final iteration value is visible after the loop.
check "persist while last value" \
      'shopt -s lastpipe; last=; printf "%s\n" a b c | while read x; do last=$x; done; echo "last=$last"'

# ---- exit status / PIPESTATUS / pipefail ----------------------------------

# 5. `exit 3` as the FIRST stage is unrelated to lastpipe (still forked/subshell
# via the `exit` builtin's normal single-command semantics), but its stage
# status (3) must still land correctly in PIPESTATUS alongside the in-process
# last stage's status (`false` => 1).
check "pipestatus exit-then-false" \
      'shopt -s lastpipe; exit 3 | false; echo "$? -- ${PIPESTATUS[@]}"'

# 6. 3-stage pipeline, last stage in-process: PIPESTATUS covers all 3 slots.
check "pipestatus 3-stage" \
      'shopt -s lastpipe; true | true | false; echo "$? -- ${PIPESTATUS[@]}"'

# 7. pipefail + lastpipe together: rightmost non-zero stage wins, including
# when the in-process last stage itself is 0.
check "pipefail with lastpipe" \
      'shopt -s lastpipe; set -o pipefail; true | false | true; echo "$? -- ${PIPESTATUS[@]}"'

# ---- function last stage + return -----------------------------------------

# 8. `return 42` from a function whose last pipeline stage runs in-process:
# the function's own $v assignment persists, $? is the returned 42, and
# PIPESTATUS records the in-process stage's real status (42), not a forked
# subshell's opaque single-slot status.
check "function return propagates" \
      'shopt -s lastpipe; f() { cat | read v; return 42; }; echo HI | f; echo "$v -- $? -- ${PIPESTATUS[@]}"'

# ---- nested lastpipe --------------------------------------------------------

# 9. An inner lastpipe pipeline inside an outer lastpipe while-loop's last
# stage: both levels' variable persistence must compose.
check "nested lastpipe" \
      'shopt -s lastpipe; printf "A\nB\n" | while read L; do printf "1\n2\n" | while read D; do echo $L$D; done; done'

# ---- closed fd 0 ------------------------------------------------------------

# 10. fd 0 closed before the pipeline: the in-process last stage's stdin must
# come from the pipe (dup'd onto fd 0 for its duration), not the closed fd.
check "closed fd0 2-stage" \
      'shopt -s lastpipe; exec 0<&-; echo x | read x; echo "x=$x"'

# 11. Same, but a 3-stage pipeline (exercises the general prev_pipe_read path,
# not just stage 0's).
check "closed fd0 3-stage" \
      'shopt -s lastpipe; echo y | cat | read y; echo "y=$y"'

# ---- negative control: lastpipe OFF ----------------------------------------

# 12. Without `shopt -s lastpipe`, the last stage still forks — $foo does NOT
# persist (matches the pre-v338 forked-last-stage behavior).
check "lastpipe off (control)" \
      'echo a b c | read foo; echo "off:$foo"'

# ---- shell-exit propagation -------------------------------------------------

# 13. `exit 9` as the in-process last stage exits the WHOLE shell (process
# exit code 9), matching bash — not just the pipeline's $?. NOTREACHED must
# not print.
check "exit from last stage exits shell" \
      'shopt -s lastpipe; exit 5 | exit 9; echo NOTREACHED'

echo ""
echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
