#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v185 (resolves L-51): a `((` at command
# position that is NOT a balanced arith block (no adjacent `))`) must lex as
# nested subshells and NOT make the arith-block scanner wander to an unrelated
# distant `))` (e.g. a later `$(( ))`). Kernel runner.sh hit this. rc 0 in bash
# → compare full stdout+exit.
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

# L-51: nested-subshell `((` (no `))`) followed by a later $(( )) / (( ).
check "subshell then dollar-arith" '((echo a) | cat); x=$((1+1)); echo "x=$x"'
check "subshell then arith cmd"    '((echo hi) >/dev/null); ((n=5)); echo "n=$n"'
check "deep subshell then arith"   '(((echo a) | cat) | cat); y=$((3*3)); echo "y=$y"'
check "two such constructs"        '((echo a)|cat); ((echo b)|cat); z=$((4+4)); echo "z=$z"'

# Controls — plain nested subshell + valid arith (unchanged).
check "plain nested subshell"      '((echo a) | cat)'
check "arith 1+2 exit"             '((1+2)); echo "rc=$?"'
check "arith grouped sum"          '(( (a=3) + (b=4) )); echo "sum=$((a+b))"'
check "arith ternary group"        '((x=(5>3)?1:0)); echo "x=$x"'
check "arith increment"            '((n=3)); ((n++)); echo "n=$n"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
