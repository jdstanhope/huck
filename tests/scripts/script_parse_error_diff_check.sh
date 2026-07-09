#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #86: a parse error at the start of a
# unit must still execute earlier already-parsed units, matching bash on stdout
# AND exit code. stderr wording diverges by design (huck "unterminated quote"
# vs bash "unexpected EOF while looking for matching"), so only stdout+rc are
# compared (stderr is sent to /dev/null).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() { local l="$1" f="$2" b h scr
  scr="$(mktemp)"; printf '%b' "$f" > "$scr"
  b=$(bash --norc "$scr" 2>/dev/null; echo "EXIT:$?")
  h=$("$HUCK_BIN" "$scr" 2>/dev/null; echo "EXIT:$?")
  rm -f "$scr"
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

check "one unit then bad"    'echo ok\n'"'"'unterminated\n'
check "two units then bad"   'echo a\necho b\n'"'"'unterminated\n'
check "heredoc then good"    'cat <<EOF\nhello\nEOF\necho after\n'
check "heredoc then bad"     'cat <<EOF\nhi\nEOF\n'"'"'unterminated\n'
check "clean multi-unit"     'echo x\necho y\n'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
