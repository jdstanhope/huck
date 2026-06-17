#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v176: a redirect on a compound-command
# pipeline stage ( subshell / { } group / if / while / case / arith ). Each case
# EXECUTES the construct (writing to a per-run temp file under our control, then
# printing its contents) and asserts identical stdout+exit under bash and huck.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

check "subshell stage > file"   "echo hi | ( cat ) > $T/a; cat $T/a"
check "group stage > file"      "printf 'x\ny\n' | { cat; } > $T/b; cat $T/b"
check "compound|compound > file" "( echo z ) | { cat; } > $T/c; cat $T/c"
check "stage 2>&1 redirect"     "echo e | ( cat ) 2>&1"
check "redirect nested in sub"  "v=\$( echo n | { cat; } > $T/d ); cat $T/d; echo \"v=[\$v]\""
check "syscall-style read"      "printf '1 a\n2 b\n' | tail -n1 | ( read n x; echo \"\$n=\$x\" ) > $T/e; cat $T/e"
check "while stage > file"      "seq 3 | while read x; do echo \"r\$x\"; done > $T/f; cat $T/f"
check "case stage > file"       "printf 'P\n' | case x in x) cat;; esac > $T/g; cat $T/g"
check "append on group stage"   "echo one > $T/h; echo two | { cat; } >> $T/h; cat $T/h"
check "regression no redirect"  "echo keep | ( cat ); echo also"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
