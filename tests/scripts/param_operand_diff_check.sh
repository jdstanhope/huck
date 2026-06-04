#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v84: ${...} operands parse as words.
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
check "alt parens+expansion"  'x=v; echo "[${x:+($x)}]"'
check "alt unset"             'unset y; echo "[${y:+($y)}]"'
check "default metachars"     'unset y; echo "[${y:-(a|b;c)}]"'
check "default unquoted split" 'unset y; for w in ${y:-a b c}; do printf "%s|" "$w"; done; echo'
check "default quoted one"    'unset y; for w in "${y:-a b c}"; do printf "%s|" "$w"; done; echo'
check "single-quoted operand" 'unset y; echo "[${y:-|;()}]"'
check "debian PS1 operand"    'debian_chroot=; PS1="${debian_chroot:+($debian_chroot)}x"; echo "$PS1"'
check "subst pattern parens"  'v="a(b)c"; echo "${v/(b)/X}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
