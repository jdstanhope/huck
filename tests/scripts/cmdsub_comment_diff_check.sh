#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v183: `#` comments inside `$( … )`. A
# word-start `#` comment runs to end-of-line; a `)` inside it must NOT close the
# substitution (huck's close-finder used to count it). Mid-word `#` stays
# literal. rc 0 in bash → compare full stdout+exit.
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
check "paren in comment"      'echo "[$(echo hi  # c with ) paren
)]"'
check "comment after open"    'echo "[$(# c with ) paren
echo yo)]"'
check "hash after semicolon"  'echo "[$(echo a;# c ) z
echo b)]"'
check "hash after pipe"       'echo "[$(echo a |# c ) z
cat)]"'
check "midword hash literal"  'echo "[$(echo a#b)]"'
check "nested paren then cmt" 'echo "[$( (echo hi)  # ) c
)]"'
check "plain cmdsub control"  'echo "[$(echo hello)]"'
check "var assign cmdsub"     'x=$(echo one  # ) two
); echo "[$x]"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
