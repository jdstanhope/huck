#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v129: a forked builtin stage must flush
# its trailing unterminated line, and a buffered parent partial line must be
# ordered before a spawned/forked child's output (M-118 + the ordering sibling).
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

check "builtin unterminated piped"   'printf "%s" abc | cat'
check "only last line unterminated"  'printf "x\ny\nz" | cat'
check "builtin piped to head"        'printf "%s" abc | head'
check "two builtins first unterm"    'printf "a\nb" | tr a-z A-Z'
check "terminated builtin unchanged" 'echo hello | cat'
check "no duplication"               'printf x; printf y | cat'
check "external ordering piped"      'printf x; /usr/bin/printf y | cat'
check "external ordering bare"       'printf x; /usr/bin/printf y'
check "builtin in subshell"          '( printf x )'
check "external in subshell"         'printf x; ( /usr/bin/printf y )'
check "loop of builtins piped"       'for i in 1 2 3; do printf "$i"; done | cat'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
