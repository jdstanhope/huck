#!/usr/bin/env bash
# Byte-identical bash<->huck harness for declare -n namerefs.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "scalar read/write"   'declare -n r=x; x=hi; echo "$r"; r=bye; echo "$x"'
chk "declare -p scalar"   'declare -n r=target; declare -p r'
chk "declare -p element"  'declare -n e=arr[0]; declare -p e'
chk "bang yields name"    'x=val; declare -n r=x; echo "$r ${!r}"'
chk "unset -n keeps tgt"  'x=v; declare -n r=x; unset -n r; echo "[$x][${r-U}]"'
chk "unset hits target"   'x=v; declare -n r=x; unset r; echo "[${x-XU}]"'
chk "chain rw"            'declare -n a=b; declare -n b=c; c=5; echo "$a"; a=9; echo "$c"'
chk "element target"      'arr=(p q r); declare -n e=arr[1]; echo "$e"; e=Q; echo "${arr[1]}"'
chk "whole array"         'arr=(a b c); declare -n r=arr; echo "${r[1]}|${r[@]}|${#r[@]}|${!r[@]}|$r"'
chk "local -n scalar"     'f(){ local -n o=$1; o=filled; }; v=e; f v; echo "$v"'
chk "local -n array"      'f(){ local -n a=$1; a+=(z); echo "${a[@]}"; }; arr=(x y); f arr; echo "${arr[@]}"'
chk "read into ref"       'declare -n r=x; printf "hi\n" | { read r; echo "$x"; }'
chk "bind then deref"     'declare -n r; r=x; declare -p r; x=1; echo "$r"'
chk "reassign target"     'x=1; y=2; declare -n r=x; echo "$r"; declare -n r=y; echo "$r"'
chk "plus n removes"      'declare -n r=x; declare +n r; declare -p r'
chk "-v on unset/set tgt" 'declare -n r=x; [[ -v r ]]&&echo S||echo U; x=1; [[ -v r ]]&&echo S2||echo U2'
chk "circular rc"         'declare -n a=b; declare -n b=a; echo "[${a-X}]"; echo "rc=$?"'
chk "self-ref declare rc" 'declare -n r=r; echo "rc=$?"'
chk "invalid target rc"   'declare -n r="a b"; echo "rc=$?"'
chk "inline scoped"       'x=orig; declare -n r=x; r=temp true; echo "$x"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
