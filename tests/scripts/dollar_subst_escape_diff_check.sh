#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v182: backslash-escaped chars in a
# ${var/pat/repl} substitution operand (pattern/replacement). The `\\\"`
# (escaped-backslash + escaped-quote) run used to crash huck with "unterminated
# quote" (kernel scripts/config line 209: V="${V//\\\"/\"}"). All cases assert
# the substitution RESULT, not just that the fragment parses. rc 0 in bash →
# compare full stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() {  # label ; fragment — assert byte-identical stdout+stderr+exit
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# The scripts/config un-escape idiom and friends.
check "unescape backslash-quote"   'V='\''a\"b'\''; echo "${V//\\\"/\"}"'
check "replace esc-quote global"   'V='\''a\"b\"c'\''; echo "${V//\\\"/X}"'
check "anchored prefix esc-quote"  'V='\''\"x'\''; echo "${V/#\\\"/Q}"'

# Escaped-delimiter / escaped-backslash controls (results unchanged by the fix).
check "escaped delimiter"          'V=a/b/c; echo "${V//\//_}"'
check "escaped backslash"          'V='\''x\y'\''; echo "${V//\\/Z}"'
check "single-slash escaped delim" 'V=a/b; echo "${V/\//_}"'

# Plain substitution + substring controls (path unaffected).
check "plain single subst"         'V=foobar; echo "${V/o/O}"'
check "plain global subst"         'V=foobar; echo "${V//o/O}"'
check "substring offset:length"    'V=abcdef; echo "${V:1:3}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
