#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v177: `$((` disambiguation. A command
# substitution whose body starts with a subshell, written glued as `$((`, must
# parse as command substitution (not arithmetic) and match bash; real arithmetic
# expansions must be unaffected. Each case EXECUTES and asserts identical
# stdout+exit.
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

# --- the bug: glued $(( subshell ) ... ) is a command substitution ---
check "subshell + 2>&1"        'echo $((echo hi) 2>&1)'
check "subshell piped"         'echo $((echo a) | tr a-z A-Z)'
check "subshell multi-cmd"     'echo $((printf X; printf Y) 2>/dev/null)'
check "subshell redirect capt" 'v=$( (printf P; printf Q) 2>/dev/null ); echo "[$v]"'
check "glued capture"          'v=$((printf m; printf n) 2>/dev/null); echo "[$v]"'

# --- regressions: real arithmetic, unaffected ---
check "plain arith"            'echo $((1+2))'
check "arith paren subexpr"    'echo $(( (1+2)*3 ))'
check "arith double paren"     'echo $(( ((4)) ))'
check "arith ternary"          'echo $((1>0?2:3))'
check "spaced subshell form"   'echo $( (echo s) 2>&1 )'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
