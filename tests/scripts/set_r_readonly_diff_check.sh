#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #229: `set -r` restricts a RUNNING
# shell but does NOT mark SHELL/PATH/HISTFILE/ENV/BASH_ENV readonly — bash
# applies those marks only when restriction engages at STARTUP (`-r`, or
# invocation as `rbash`). Every OTHER restriction applies either way.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
norm() { sed 's|^[^:]*: line |PROG: line |; s|/tmp/x[0-9]*|/tmp/xPID|'; }
check() {  # runtime `set -r`
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
checkr() {  # STARTUP restriction, via the -r flag
    local label="$1" frag="$2" b h
    b=$(bash -r -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -r -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- set -r leaves the five variables writable ------------------------------
check "PATH"          'set -r; PATH=/x; echo "rc=$? $PATH"'
check "SHELL"         'set -r; SHELL=/x; echo "rc=$? $SHELL"'
check "ENV"           'set -r; ENV=/x; echo "rc=$? $ENV"'
check "BASH_ENV"      'set -r; BASH_ENV=/x; echo "rc=$? $BASH_ENV"'
check "HISTFILE"      'set -r; HISTFILE=/x; echo "rc=$? $HISTFILE"'
check "no -r attr"    'set -r; declare -p PATH | head -c 22'
check "in a subshell" 'set -r; ( PATH=/x; echo inner=$? )'
check "export form"   'set -r; export PATH=/x; echo "rc=$? $PATH"'
check "unset"         'set -r; unset ENV; echo rc=$?'

# --- but every other restriction still applies immediately ------------------
check "cd"            'set -r; cd /tmp; echo rc=$?'
check "slash command" 'set -r; /bin/echo hi; echo rc=$?'
check "redirect out"  'set -r; echo hi > /tmp/x$$; echo rc=$?'
check "append out"    'set -r; echo hi >> /tmp/x$$; echo rc=$?'

# --- startup restriction DOES mark them readonly ----------------------------
checkr "startup PATH"     'PATH=/x; echo rc=$?'
checkr "startup SHELL"    'SHELL=/x; echo rc=$?'
checkr "startup attr"     'declare -p PATH | head -c 23'
checkr "startup cd"       'cd /tmp; echo rc=$?'
checkr "startup ordinary" 'x=1; echo "rc=$? $x"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
