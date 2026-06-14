#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the coproc reserved word (v157).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() { local l="$1" f="$2" b h
  b=$(printf '%s\n' "$f" | timeout 10 bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | timeout 10 "$HUCK_BIN" 2>&1; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

check "anon roundtrip"   'coproc { read l; echo "got:$l"; }; echo hi >&"${COPROC[1]}"; read r <&"${COPROC[0]}"; echo "$r"'
check "named roundtrip"  'coproc MYP { read l; echo "e:$l"; }; echo yo >&"${MYP[1]}"; read r <&"${MYP[0]}"; echo "$r"'
check "two lines"        'coproc { while read l; do echo "L:$l"; done; }; printf "a\nb\n" >&"${COPROC[1]}"; read x <&"${COPROC[0]}"; read y <&"${COPROC[0]}"; echo "$x $y"'
check "pid==bang"        'coproc cat; [ "$COPROC_PID" = "$!" ] && echo match'
check "named array size" 'coproc cat; echo "${#COPROC[@]}"'
check "autounset on wait" 'coproc { :; }; wait "$COPROC_PID" 2>/dev/null; echo "[${COPROC[0]-unset}]"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
