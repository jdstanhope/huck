#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v194: `trap … 0` (numeric 0 ≡ EXIT).
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
check "register 0"   "trap 'echo EX' 0; echo body"
check "0 plus sig"   "trap 'echo EX' 0 2; echo body"
check "ignore '' 0"  "trap '' 0; echo body"
check "reset - 0"    "trap 'echo EX' 0; trap - 0; echo body"
check "trap -p 0"    "trap 'echo A' 0; trap -p 0"
check "EXIT name"    "trap 'echo EX' EXIT; echo body"
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
