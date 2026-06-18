#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v184: `((` at command position is
# nested subshells `( (…) )` when there is no matching `))` (huck used to
# hard-error "unterminated '((' arithmetic block": kernel zdiff / runner.sh).
# A `((` that DOES close as `))` stays arithmetic. rc 0 in bash → compare full
# stdout+exit.
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

# `((` as nested subshells (no matching `))`).
check "nested subshell + pipe"  '((echo a) | cat)'
check "nested + redir + seq"    '((echo a) >/dev/null; echo b)'
check "nested pipe with redir"  '((echo hi >&2) 2>&1 | cat)'
check "deeply nested spaced"    '(((  echo a ) ) )'
check "nested seq subshell"     '( (echo a; echo b) )'

# Arithmetic controls (DO close as `))` — unchanged).
check "arith true"              '((1+2)) && echo arith-true'
check "arith assign"            '((x=5)); echo "x=$x"'
check "arith increment"         '((n=3)); ((n++)); echo "n=$n"'
check "arith false exit"        '((0)); echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
