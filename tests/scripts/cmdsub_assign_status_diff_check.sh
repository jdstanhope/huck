#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v126: a bare assignment's $? = the last
# command substitution in its RHS (or 0). File-arg execution (L-27).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>/dev/null; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "false sub"        'x=$(false); echo $?'
check "exit7 sub"        'x=$(exit 7); echo $?'
check "plain zero"       'x=5; echo $?'
check "two assigns last" 'x=$(false) y=$(exit 2); echo $?'
check "two subs one rhs" 'x="$(false)$(exit 5)"; echo $?'
check "dollarq snapshot" 'false; x=$?; echo $x'
check "local keeps 0"    'f(){ local v=$(exit 9); echo $?; }; f'
check "prefix keeps cmd" 'x=$(exit 3) true; echo $?'
check "append sub"       'x=a; x+=$(exit 4); echo $?'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
