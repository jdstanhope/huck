#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v141: cmdsub/arith/backtick in prompt
# expansion, exercised via ${var@P} (the prompt expander) through `-c`.
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
check "cmdsub"        'v='\''$(echo CMDSUB)'\''; echo "${v@P}"'
check "arith"         'v='\''$((6*7))'\''; echo "${v@P}"'
check "arith parens"  'v='\''$(( (1+2)*3 ))'\''; echo "${v@P}"'
check "cmdsub mid"    'v='\''pre-$(echo mid)-post'\''; echo "${v@P}"'
check "cmdsub nested" 'v='\''$(echo $(echo nested))'\''; echo "${v@P}"'
check "backtick"      'v='\''`echo bt`'\''; echo "${v@P}"'
check "cmdsub+var"    'x=VAL; v='\''[$x]$(echo Y)'\''; echo "${v@P}"'
check "trailing nl"   'v='\''$(printf "a\n\n")|'\''; echo "${v@P}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
