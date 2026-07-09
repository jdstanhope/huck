#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the v276 `read`-builtin cluster.
# B-02: EOF exit status (1 on EOF, even with partial data) + variable
# clearing (assignment always runs, so vars are reset to "" on EOF rather
# than left stale). Compares stdout+rc; fragments run as a script piped on
# stdin (matches read_u_diff_check.sh) so the fd-0 read path is exercised.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() { local l="$1" f="$2" b h
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# --- B-02: EOF exit status + variable clearing ---
check "eof-partial rc"     'printf abc | { read x; echo "rc=$? [$x]"; }'          # bash rc1 [abc]
check "eof-empty clears"   'printf "" | { x=OLD; read x; echo "rc=$? [$x]"; }'    # bash rc1 []
check "eof-multi clears"   'printf "" | { x=A y=B; read x y; echo "rc=$? [$x][$y]"; }' # rc1 [][]
check "full-line rc0"      'printf "abc\n" | { read x; echo "rc=$? [$x]"; }'      # rc0 [abc]

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
