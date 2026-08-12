#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `read -u FD` (L-34).
# Compares stdout+rc for the happy paths; for the error paths compares
# stderr too but NORMALIZES the leading program-name/path (bash prints
# `bash: line N: read:` vs huck's `<huckpath>: line N: read:`) by keeping
# only the part from `read:` onward.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# Keep everything from `read:` onward on any diagnostic line so the leading
# program-name/path/line-number prefix doesn't cause a spurious mismatch.
norm() { sed 's/.*\(read:.*\)/\1/'; }

# stdout + rc only (no diagnostics expected).
check() { local l="$1" f="$2" b h
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# stdout + rc + NORMALIZED stderr (error paths).
check_err() { local l="$1" f="$2" b h
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1 1>/dev/null | norm; printf '%s\n' "$f" | bash --norc --noprofile >/dev/null 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | "$HUCK_BIN" 2>&1 1>/dev/null | norm; printf '%s\n' "$f" | "$HUCK_BIN" >/dev/null 2>&1; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

check     "valid fd + word-split"  'f=$(mktemp); printf "hello world extra\n">"$f"; exec 3<"$f"; read -u 3 a b; echo "[$a][$b] rc=$?"; exec 3<&-; rm -f "$f"'
check     "bundled -u3"            'f=$(mktemp); printf "hello world extra\n">"$f"; exec 3<"$f"; read -u3 a; echo "[$a]"; exec 3<&-; rm -f "$f"'
check     "fd 0 = stdin"           'printf "line0\n" | { read -u 0 v; echo "[$v]"; }'
check     "while read -u N drain"  'f=$(mktemp); printf "a\nb\nc\n">"$f"; exec 4<"$f"; while read -u 4 l; do echo "got:$l"; done; echo done; exec 4<&-; rm -f "$f"'
check     "no over-consume"        'f=$(mktemp); printf "first\nsecond\nthird\n">"$f"; exec 5<"$f"; read -u 5 x; echo "x=$x"; while read -u 5 y; do echo "rest=$y"; done; exec 5<&-; rm -f "$f"'
check_err "non-numeric fd"         'read -u xyz v'
check_err "unopened fd 9"          'read -u 9 v'

# --- #546: a failed read(2) is reported as `read error: <fd>: <strerror>`,
#     naming the descriptor read from. huck printed the strerror alone, so a
#     script reading from several fds could not tell which one failed. `-u`
#     validates the fd first, so reaching this needs an fd that IS open but
#     cannot be read: a write-only one, or a directory.
check_err "closed stdin"           'exec 0<&-; read v'
# NOT a row: `exec 0>/dev/null; read v`. This harness pipes the script into the
# shell on stdin, so REPLACING fd 0 breaks the script reader itself — bash
# reports `error reading input file` and exits 2 before `read` ever runs. Under
# `-c` both shells agree (`read: read error: 0: Bad file descriptor`).
check_err "write-only -u fd"       'exec 4>/dev/null; read -u 4 v'
check_err "directory on stdin"     'read v < /'
check_err "directory on -u fd"     'exec 5</etc; read -u 5 v'
check_err "closed stdin in pipe"   'echo x | { exec 0<&-; read v; }'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
