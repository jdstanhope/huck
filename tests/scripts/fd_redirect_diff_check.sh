#!/usr/bin/env bash
# Byte-identical bash<->huck harness for arbitrary-fd (fd>2) redirections (v156).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() { local l="$1" f="$2" b h
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

check "exec hold/write/close"  'f=$(mktemp); exec 3>"$f"; echo x >&3; exec 3>&-; cat "$f"; rm -f "$f"'
check "exec read via <&3"      'f=$(mktemp); printf "a\nb\n">"$f"; exec 3<"$f"; read u <&3; read v <&3; echo "$u$v"; exec 3<&-; rm -f "$f"'
check "L-08 2>&1 >file (cmd)"   'f=$(mktemp); printf "%s\n" out 2>&1 >"$f"; echo "file=[$(cat "$f")]"; rm -f "$f"'
check "L-08 >file 2>&1 (cmd)"   'f=$(mktemp); { echo out; echo err >&2; } >"$f" 2>&1; echo "file=[$(cat "$f")]"; rm -f "$f"'
check "L-08 builtin 2>&1 >file" 'f=$(mktemp); echo x 2>&1 >"$f"; echo "file=[$(cat "$f")]"; rm -f "$f"'
check "fd swap stdout/stderr"   'sh -c "echo O; echo E >&2" 3>&1 1>&2 2>&3 3>&- 2>/dev/null'
check "<> read-write"           'f=$(mktemp); printf abc>"$f"; exec 3<>"$f"; printf X>&3; exec 3>&-; cat "$f"; rm -f "$f"'
check "named {fd} >=10 inproc"  'f=$(mktemp); { printf z >&$fd; } {fd}>"$f"; [ "$fd" -ge 10 ] && echo okfd; cat "$f"; rm -f "$f"'
check "10>>file append"         'f=$(mktemp); printf head>"$f"; exec 10>>"$f"; printf body>&10; exec 10>&-; cat "$f"; rm -f "$f"'
check "external fd>2 inherit"   'f=$(mktemp); sh -c "echo hi >&3" 3>"$f"; cat "$f"; rm -f "$f"'
check "pipeline stage fd>2"     'f=$(mktemp); sh -c "echo ps >&3" 3>"$f" | cat; echo "p=[$(cat "$f")]"; rm -f "$f"'
check "bad source fd EBADF"     '(echo x >&9) 2>/dev/null; echo "rc=$?"'
check "missing input file"      '(exec 3</no/such_xyz) 2>/dev/null; echo "rc=$?"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
