#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #411: `wait`'s operand error model.
# Every operand is processed and diagnosed where it stands (bash never stops at
# the first bad one), the builtin's status is the LAST operand's, and `-n` has
# its own model where anything unresolvable is "no such job".
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
norm() { sed 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- an unresolvable job spec is 127, not 1 ---------------------------------
check "unknown spec"      'wait %1; echo rc=$?'
check "unknown spec --"   'wait -- %1; echo rc=$?'
check "unknown pid"       'wait 99999; echo rc=$?'

# --- every operand is diagnosed, and the LAST one sets the status -----------
check "two bad specs"     'wait %1 %2; echo rc=$?'
check "bad word then spec" 'wait abc %1; echo rc=$?'
check "spec then bad word" 'wait %1 abc; echo rc=$?'
check "pid, word, pid"    'wait 12345 abc 99999; echo rc=$?'
check "bad word alone"    'wait abc; echo rc=$?'
check "empty word alone"  'wait ""; echo rc=$?'

# --- what counts as a pid word ----------------------------------------------
# bash requires a leading DIGIT, then legal_number: `0` is a pid word (and is
# reported as no child — never handed to waitpid, where 0 means "my group"),
# while `+12` and ` 12` are not pid words at all.
check "pid zero"          'wait 0; echo rc=$?'
check "pid zero + other"  'wait 0 12345; echo rc=$?'
check "leading plus"      'wait +12; echo rc=$?'
check "leading space"     'wait " 12"; echo rc=$?'
check "trailing space"    'wait "12345 "; echo rc=$?'
check "digits then word"  'wait 12abc; echo rc=$?'

# --- -n: everything unresolvable is "no such job" ---------------------------
check "-n bad word"       'wait -n abc; echo rc=$?'
check "-n unknown pid"    'wait -n 99999; echo rc=$?'
check "-n unknown spec"   'wait -n %1; echo rc=$?'
check "-n spec then word" 'wait -n %1 abc; echo rc=$?'
check "-n word then spec" 'wait -n abc %1; echo rc=$?'
check "-n pid then spec"  'wait -n 12345 %1; echo rc=$?'

# --- live jobs still resolve (nothing regressed) ----------------------------
check "live job by spec"  'sleep 0.2 & wait %1; echo rc=$?'
check "live job by pid"   'sleep 0.2 & p=$!; wait $p; echo rc=$?'
check "-n live job"       'sleep 0.2 & wait -n %1; echo rc=$?'
check "-n live + unknown" 'sleep 0.2 & wait -n 99999 %1; echo rc=$?'
check "exit status kept"  'bash -c "exit 7" & wait %1; echo rc=$?'
check "two live jobs"     'sleep 0.1 & sleep 0.2 & wait %1 %2; echo rc=$?'
check "bare wait"         'sleep 0.1 & wait; echo rc=$?'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
