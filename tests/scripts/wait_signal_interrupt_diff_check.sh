#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a TRAPPED signal interrupting `wait`
# (#453). bash runs the action and returns 128+n immediately, leaving the
# remaining jobs running; it does not resume waiting.
#
# huck used to finish the wait first, which was invisible to a diff harness in
# the common case (same output, later) — but two things ARE observable and are
# what this pins: the 128+n status, and the fact that the other jobs are still
# Running afterwards.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 20 bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(timeout 20 "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- a trapped signal interrupts and yields 128+n --------------------------
check "bare wait"        'trap "echo caught" USR1; sleep 1 & ( sleep 0.2; kill -USR1 $$ ) & wait; echo "rc=$?"'
check "wait \$!"         'trap "echo caught" USR1; sleep 1 & ( sleep 0.2; kill -USR1 $$ ) & wait $!; echo "rc=$?"'
check "wait %1"          'trap "echo caught" USR1; sleep 1 & ( sleep 0.2; kill -USR1 $$ ) & wait %1; echo "rc=$?"'
check "USR2 is 128+12"   'trap "echo caught" USR2; sleep 1 & ( sleep 0.2; kill -USR2 $$ ) & wait; echo "rc=$?"'
check "action output"    'trap "echo GOT" USR1; sleep 1 & ( sleep 0.2; kill -USR1 $$ ) & wait; echo "after rc=$?"'

# --- the remaining job keeps running ---------------------------------------
# NOT covered: `jobs | wc -l` right after the interrupt. bash still lists the
# just-exited signaller, huck has already pruned it — a job-table timing
# divergence unrelated to the interrupt itself (#475).
check "second wait works" 'trap "echo caught" USR1; sleep 0.4 & ( sleep 0.2; kill -USR1 $$ ) & wait; wait; echo "rc=$?"'

# --- signals that must NOT interrupt ---------------------------------------
check "ignored signal"   'trap "" USR1; sleep 0.4 & ( sleep 0.2; kill -USR1 $$ ) & wait; echo "rc=$?"'
check "no signal at all" 'sleep 0.3 & wait; echo "rc=$?"'
check "no signal, spec"  'sleep 0.3 & wait %1; echo "rc=$?"'
check "job status kept"  'sh -c "exit 7" & wait $!; echo "rc=$?"'
# NOT covered: install-then-`trap - USR1`, where the default disposition should
# come back and kill the shell. huck survives it — filed as #474, a reset bug
# rather than a wait bug.

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
