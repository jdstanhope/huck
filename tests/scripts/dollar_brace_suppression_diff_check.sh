#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #319: a `{` directly after an unquoted
# `$` does not open a brace expansion. bash brace-expands the RAW word, where
# `${` reads as the start of a parameter expansion and is skipped — so
# `echo $${x,y}` prints `<pid>{x,y}`, while `$?{x,y}` and `${HOME}{x,y}` expand
# normally. `$$` is the only construct that can leave a bare `$` against a `{`.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# The pid differs between the two shells, so collapse any 4+ digit run.
norm() { sed 's/[0-9]\{4,\}/PID/g'; }
check() {
    local label="$1" frag="$2" b h
    b=$(HOME=/h/u bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$(HOME=/h/u "$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- suppressed after $$ ----------------------------------------------------
check "bare"              'echo $${x,y}'
check "with suffix"       'echo $${a,b}c'
check "with prefix"       'echo pre$${a,b}'
check "range form"        'echo $${1..3}'
check "nested braces"     'echo $${a,{b,c}}'
check "two of them"       'echo $${a,b}$${c,d}'
check "pid then param"    'a=Z; echo $${a}'
check "three dollars"     'echo $$${a,b}'

# --- NOT suppressed anywhere else -------------------------------------------
check "after \$?"          'echo $?{x,y}'
check "after \$#"          'echo $#{x,y}'
check "after \${HOME}"     'echo ${HOME}{x,y}'
check "after a literal"   'echo a{x,y}'
check "after a var"       'x=1; echo $x{a,b}'
check "quoted dollar"     'echo "$"{a,b}'
check "escaped dollar"    'echo \${a,b}'
check "dollar then space" 'echo $$ {a,b}'
check "plain brace"       'echo {a,b}'
check "quoted braces"     'echo "{a,b}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
