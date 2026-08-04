#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `trap` / `trap -p` listing format:
# a real signal prints with its SIG prefix (`SIGUSR1`, unlike `kill -l`, which
# lists bare names), the pseudo-signals do not, and the order is bash's table
# walk — EXIT (signal 0) first, then real signals by NUMBER, then DEBUG, ERR,
# RETURN.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- the SIG prefix ---------------------------------------------------------
check "by name"        'trap "echo x" USR1; trap'
check "by SIG name"    'trap "echo x" SIGUSR1; trap'
check "by number"      'trap "echo x" 15; trap'
check "ignored"        'trap "" INT; trap'
check "-p one signal"  'trap "echo x" USR1; trap -p USR1'
check "-p by SIG name" 'trap "echo x" USR1; trap -p SIGUSR1'
check "-p ignored"     'trap "" INT; trap -p INT'

# --- pseudo-signals keep their bare names -----------------------------------
check "EXIT"           'trap "echo x" EXIT; trap -p EXIT'
check "DEBUG"          'trap "echo x" DEBUG; trap -p DEBUG'
check "ERR"            'trap "echo x" ERR; trap -p ERR'
check "RETURN"         'trap "echo x" RETURN; trap -p RETURN'

# --- ordering: EXIT, signals by number, then DEBUG/ERR/RETURN ---------------
check "full table"     'trap "echo a" USR1; trap "echo b" EXIT; trap "echo c" ERR; trap "echo d" DEBUG; trap "echo e" RETURN; trap "" INT; trap "echo f" 15; trap'
check "signals only"   'trap "echo x" TERM; trap "echo y" HUP; trap "echo z" USR2; trap'
check "reverse install" 'trap "echo z" USR2; trap "echo y" HUP; trap "echo x" TERM; trap'
check "pseudo only"    'trap "echo c" ERR; trap "echo d" DEBUG; trap "echo e" RETURN; trap'

# --- quoting of the action --------------------------------------------------
check "quote in action" "trap \"echo it's\" USR1; trap"
check "empty table"     'trap'
check "reset one"       'trap "echo x" USR1; trap - USR1; trap'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
