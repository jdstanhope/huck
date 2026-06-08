#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v112: the arithmetic comma operator
# (M-108). `L , R` -> eval L (side effects) then R; value is R; lowest precedence.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "(( a=1,b=2 ))"        '(( a=1, b=2 )); echo "$a $b"'
check "value is last"        'echo $((1, 2, 3))'
check "comma in parens"      'echo $(( (1,2) + 3 ))'
check "comma below assign"   'a=9; echo $(( a=1, 2 )); echo "$a"'
check "nested comma"         'echo $(( (1,2),3 ))'
check "c-for comma"          'for ((i=0,j=0; i<3; i++,j++)); do echo "$i:$j"; done'
check "comma side effects"   'echo $(( x=5, x+1 )); echo "$x"'
check "spaces around comma"  'echo $(( 1 , 2 , 3 ))'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
