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
# #223: `>&word` / `1>&word` whose word is a single non-numeric, non-`-` field
# is a synonym for `&>word` (redirect BOTH stdout+stderr to that file), NOT a
# `bad fd` error. A numeric/`-` word stays a real dup/close.
check "223 >&file both streams" 'f=$(mktemp); sh -c "echo O; echo E >&2" >&"$f"; sort "$f"; rm -f "$f"'
check "223 1>&file both"        'f=$(mktemp); sh -c "echo O; echo E >&2" 1>&"$f"; sort "$f"; rm -f "$f"'
check "223 builtin >&file"      'f=$(mktemp); echo hi >&"$f"; cat "$f"; rm -f "$f"'
check "223 >&file var word"     'f=$(mktemp); w="$f"; echo hi >&"$w"; cat "$f"; rm -f "$f"'
check "223 >&2 numeric dup"     'echo O >&2 2>/dev/null; echo "rc=$?"'
# `>&$x` where x expands to `-` closes the fd (like a literal `>&-`); it must
# NOT be taken as a filename `-`. (fd2 redirect on the GROUP, not the builtin,
# to avoid the separate pre-existing builtin close+`2>/dev/null` ordering bug.)
check "223 >&\$x x=- closes"    'x=-; { echo hi >&$x; } 2>/dev/null; echo "rc=$?"'
check "223 >&\$x x=- no file"   'x=-; { echo hi >&$x; } 2>/dev/null; test -e ./- && echo LEAK || echo clean; rm -f ./-'
check "223 >&\$comsub once"     'f=$(mktemp); echo hi >&"$(echo side >&2; echo "$f")" 2>&1 1>/dev/null; echo "n=$(grep -c side "$f" 2>/dev/null)$(cat "$f")"; rm -f "$f"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
