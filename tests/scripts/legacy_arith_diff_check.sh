#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v188: legacy $[ … ] arithmetic
# expansion (bash's deprecated synonym for $(( … ))).
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

check "basic add"        'echo $[3+4]'
check "no-glob star"     'echo $[ 2 * 5 ]'
check "exponent paren"   'len=4; echo $[ 2**(len*2)-1 ]'
check "ternary"          'IP6=6; echo $[ IP6 ? 128 : 32 ]'
check "base prefix"      'd=ff; echo $[ 16#$d ]'
check "array subscript"  'a=(5 6); echo $[a[1]+1]'
check "rbracket in dpe"  'a=(9); echo $[ ${a[0]} + 1 ]'
check "nested cmdsub"    'echo $[$(echo 3)+1]'
check "nested dollarvar" 'x=4; echo $[${x}+1]'
check "nested legacy"    'echo $[ $[2+3] * 2 ]'
check "in dquotes"       'echo "$[1+2]"'
check "comma"            'echo $[1,2,3]'
check "division"         'echo $[10/3]'
check "neg + assign"     'echo $[ -5 + 1 ]'
check "control $(( ))"   'echo $((1+1))'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
