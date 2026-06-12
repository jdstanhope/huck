#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v145: export -p / export -n.
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
check "p lists declare -x"  'export ZA=1 ZB=two; export -p | grep -E "declare -x Z[AB]="'
check "bare export format"  'export ZC=hi; export | grep "declare -x ZC="'
check "p readonly export"   'export ZR=1; readonly ZR; export -p | grep "ZR="'
check "n unexport keeps"    'export ZD=keep; export -n ZD; declare -p ZD'
check "n assign+unexport"   'export ZE=1; export -n ZE=2; declare -p ZE'
check "n readonly keeps"    'export ZF=1; readonly ZF; export -n ZF; declare -p ZF'
check "n unset noop"        'export -n ZNOPE; echo "rc=$?"'
check "pn unexports"        'export ZG=1; export -pn ZG; declare -p ZG'
check "p operand exports"   'ZH=1; export -p ZH; declare -p ZH | grep -o "declare -x ZH"'
check "invalid flag rc2"    'export -z 2>/dev/null; echo "rc=$?"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
