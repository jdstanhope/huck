#!/usr/bin/env bash
# Byte-identical bash<->huck harness for declare/local -l / -u case-fold attrs.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "lower scalar"        'declare -l x; x=ABCdef; echo "$x"'
chk "upper scalar"        'declare -u x; x=ABCdef; echo "$x"'
chk "inline upper"        'declare -u x=hello; echo "$x"'
chk "not retroactive"     'x=ABC; declare -l x; echo "$x"'
chk "same-cmd cancel"     'declare -lu x; x=AbC; echo "$x"'
chk "same-cmd cancel -p"  'declare -lu x=AbC; declare -p x'
chk "last wins l->u"      'declare -l x; declare -u x; x=AbC; echo "$x"'
chk "last wins u->l"      'declare -u x; declare -l x; x=AbC; echo "$x"'
chk "remove +l"           'declare -l x; x=ABC; declare +l x; x=DEF; echo "$x"'
chk "array each elem"     'declare -l arr; arr=(ABC DeF GHI); echo "${arr[@]}"'
chk "array elem assign"   'declare -l arr; arr=(ABC DeF GHI); arr[1]=XYZ; echo "${arr[1]}"'
chk "array append"        'declare -u arr; arr=(ab); arr+=(cd ef); echo "${arr[@]}"'
chk "assoc value not key" 'declare -lA m; m[Key]=VALUE; echo "${m[Key]}"; echo "${!m[@]}"'
chk "integer then upper"  'declare -iu x; x=3+4; echo "$x"'
chk "scalar append fold"  'declare -l x; x=ABC; x+=DEF; echo "$x"'
chk "local lower scope"   'f(){ local -l v=HELLO; echo "$v"; }; f; echo "${v:-unset}"'
chk "local upper scope"   'f(){ local -u v=lo; echo "$v"; }; f; echo "${v:-unset}"'
chk "declare -p lower"    'declare -l x; x=ab; declare -p x'
chk "declare -p flags"    'declare -irxl a=1; declare -p a'
chk "typeset -u"          'typeset -u x=abc; echo "$x"'
chk "plus on cancelled"   'declare -lu x; declare +u x; x=AbC; echo "$x"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
