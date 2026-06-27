#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v231 C: shopt expand_aliases in file mode.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-aliasx.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "def then use"     'shopt -s expand_aliases; alias foo="echo HELLO"; foo'
checkf "alias with arg"   'shopt -s expand_aliases; alias ll="echo LL"; ll /usr'
checkf "no shopt = literal" 'alias foo="echo HELLO"; foo'
checkf "unalias then use" 'shopt -s expand_aliases; alias foo="echo HI"; foo; unalias foo; foo'
checkf "trailing space"   'shopt -s expand_aliases; alias a="b "; alias b="echo"; a hi'
checkf "redefine"         'shopt -s expand_aliases; alias g="echo one"; g; alias g="echo two"; g'
checkf "set -v echo raw"  'set -v; shopt -s expand_aliases; alias ll="echo LL"; ll /usr'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
