#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #423: fg/bg/jobs take a job spec with
# or without its leading `%` — `1` is job 1, `foo` is a command-prefix match —
# while the pid-taking builtins (kill/wait/disown) must keep reading a bare
# number as a PID.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
norm() { sed -E 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
J='set -m; sleep 3 & '
R='; kill -KILL %1 2>/dev/null; wait 2>/dev/null'

# --- a bare number is a job number ------------------------------------------
check "bg 1"            "${J}bg 1; echo rc=\$?${R}"
check "jobs 1"          "${J}jobs 1; echo rc=\$?${R}"
check "jobs 1 vs %1"    "${J}jobs 1 > /tmp/a\$\$; jobs %1 > /tmp/b\$\$; cmp -s /tmp/a\$\$ /tmp/b\$\$ && echo same; rm -f /tmp/a\$\$ /tmp/b\$\$${R}"
check "bg 9 no such"    "${J}bg 9; echo rc=\$?${R}"
check "jobs 9 no such"  "${J}jobs 9; echo rc=\$?${R}"

# --- a bare word is a command-prefix match ----------------------------------
check "bg by prefix"    "${J}bg sleep; echo rc=\$?${R}"
check "jobs by prefix"  "${J}jobs sleep; echo rc=\$?${R}"
check "bg bad prefix"   "${J}bg foo; echo rc=\$?${R}"
check "jobs bad prefix" "${J}jobs abc; echo rc=\$?${R}"

# --- with no jobs at all ----------------------------------------------------
check "bg 1 no jobs"    'set -m; bg 1; echo rc=$?'
check "jobs 1 no jobs"  'set -m; jobs 1; echo rc=$?'
check "jobs 99999"      'jobs 99999; echo rc=$?'

# --- the %-forms are unchanged ----------------------------------------------
check "bg %1"           "${J}bg %1; echo rc=\$?${R}"
check "jobs %1"         "${J}jobs %1; echo rc=\$?${R}"
check "jobs %sleep"     "${J}jobs %sleep; echo rc=\$?${R}"

# --- pid-taking builtins still read a bare number as a PID ------------------
# `disown 1` is pid 1, NOT job 1: bash says "no such job" even with job 1 live.
check "disown 1 is a pid" "${J}disown 1; echo rc=\$?${R}"
check "kill -0 1"         "${J}kill -0 1; echo rc=\$?${R}"
check "wait 1"            "${J}wait 1; echo rc=\$?${R}"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
