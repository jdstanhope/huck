#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v101: subshell inside command substitution.
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

check "subshell"       'echo "$( (echo a) )"'
check "subshell ||"    'echo "$( (echo a) || echo b )"'
check "subshell pipe"  'echo "$(echo a | (cat))"'
check "subshell semis" 'echo "$( (exit 3); echo done )"'
check "nested arith"   'echo "$( echo $((1 + 2)) )"'
check "in default"     'echo "${x:-$( (echo d) )}"'
check "in array lit"   'a=( "$( (echo x) )" ); echo "${a[0]}"'
check "plain regress"  'echo "$(echo a)"'
check "nested regress" 'echo "$(echo "$(echo b)")"'
check "backtick sub"   'echo "`(echo a)`"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
