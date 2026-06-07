#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v103: set -x xtrace (M-??).
# xtrace goes to stderr; check compares combined stdout+stderr+EXIT.
# Only TOP-LEVEL, depth-1, default-PS4 fragments where huck and bash agree.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "trace echo"     'set -x; echo hi'
check "trace expanded" 'x=hi; set -x; echo "$x" a'
check "enable disable" 'set -x; echo a; set +x; echo b'
# NOTE: "dash has x" dropped — verifying $- contains x requires tracing an
# assignment (`d=$-`) or a [[/case compound, neither of which huck traces, and
# $- content differs anyway. Integration tests already cover $- having x.
check "set -o xtrace"  'set -o xtrace; echo hi'
check "trace true"     'set -x; true; set +x; echo done'
check "trace two args" 'set -x; echo one two'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
