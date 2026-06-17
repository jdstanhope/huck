#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v174: command substitution inside array
# literals — backtick elements, $()-with-quotes elements, and mixtures. Each case
# EXECUTES the assignment and prints element values/count, asserting identical
# output under bash and huck.
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

check "single backtick"      'a=(`echo hi`); echo "${a[0]} n=${#a[@]}"'
check "two backticks"        'a=(`echo one` `echo two`); echo "${a[*]} n=${#a[@]}"'
check "backtick multi-word"  'a=(`echo a b c`); echo "n=${#a[@]} [${a[*]}]"'
check "mixed dollar+backtick" 'a=($(echo x) `echo y`); echo "${a[*]} n=${#a[@]}"'
check "dollarparen paren-in-quote" 'a=($(echo ")")); echo "[${a[0]}] n=${#a[@]}"'
check "backtick paren-in-quote"    'a=(`echo ")"`); echo "[${a[0]}] n=${#a[@]}"'
check "IFS newline backtick"  $'IFS=$\'\\n\' a=(`printf \'p\\nq\'`); echo "n=${#a[@]} [${a[0]}][${a[1]}]"'
check "nested backtick in sub" 'a=($(echo `echo hi`)); echo "${a[0]}"'
check "regression dollarparen" 'a=($(echo hi) plain); echo "${a[*]} n=${#a[@]}"'
check "literal next to backtick" 'a=(pre`echo X`post); echo "${a[0]} n=${#a[@]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
