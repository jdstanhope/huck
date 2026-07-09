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

# Keep everything from `read:` onward on any diagnostic line so the leading
# program-name/path/line-number prefix doesn't cause a spurious mismatch
# (bash prints `bash: line N: read:` vs huck's `<huckpath>: line N: read:`).
# Same normalize-and-compare pattern as read_u_diff_check.sh.
norm() { sed 's/.*\(read:.*\)/\1/'; }

# stdout + rc + NORMALIZED stderr (error paths): compares the message tail too.
check_err() { local l="$1" f="$2" b h
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1 | norm; printf '%s\n' "$f" | bash --norc --noprofile >/dev/null 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | "$HUCK_BIN" 2>&1 | norm; printf '%s\n' "$f" | "$HUCK_BIN" >/dev/null 2>&1; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

# --- B-02: EOF exit status + variable clearing ---
check "eof-partial rc"     'printf abc | { read x; echo "rc=$? [$x]"; }'          # bash rc1 [abc]
check "eof-empty clears"   'printf "" | { x=OLD; read x; echo "rc=$? [$x]"; }'    # bash rc1 []
check "eof-multi clears"   'printf "" | { x=A y=B; read x y; echo "rc=$? [$x][$y]"; }' # rc1 [][]
check "full-line rc0"      'printf "abc\n" | { read x; echo "rc=$? [$x]"; }'      # rc0 [abc]

# --- B-03: last-field trailing IFS delimiter ---
check "b03 :a:b: 3v"   'printf ":a:b:\n"  | { IFS=: read x y z; echo "[$x][$y][$z]"; }'
check "b03 :a:b:: 3v"  'printf ":a:b::\n" | { IFS=: read x y z; echo "[$x][$y][$z]"; }'
check "b03 a:b:c:d 3v" 'printf "a:b:c:d\n"| { IFS=: read x y z; echo "[$x][$y][$z]"; }'
check "b03 a: 2v"      'printf "a:\n"     | { IFS=: read x y; echo "[$x][$y]"; }'
check "b03 a:: 2v"     'printf "a::\n"    | { IFS=: read x y; echo "[$x][$y]"; }'
check "b03 a::: 2v"    'printf "a:::\n"   | { IFS=: read x y; echo "[$x][$y]"; }'
check "b03 a:b:: 2v"   'printf "a:b::\n"  | { IFS=: read x y; echo "[$x][$y]"; }'
check "b03 mixed"      'printf "a:b: \n"  | { IFS=": " read x y; echo "[$x][$y]"; }'
check "b03 single var" 'printf "a:b:\n"   | { IFS=: read x; echo "[$x]"; }'
check "b03 ws trail"   'printf "a b  \n"  | { read x y; echo "[$x][$y]"; }'

# --- M-162: -n N / -N N character-counted reads ---
check "n3 count"     'printf "hello" | { read -n 3 x; echo "rc=$? [$x]"; }'       # rc0 [hel]
check "n5 stop-nl"   'printf "ab\ncd" | { read -n 5 x; echo "rc=$? [$x]"; }'      # rc0 [ab]
check "N5 across-nl" 'printf "ab\ncd" | { read -N 5 x; echo "rc=$?"; echo "[$x]"; }'  # x="ab\ncd"
check "n0"           'printf "hi" | { read -n 0 x; echo "rc=$? [$x]"; }'          # rc0 []
check "n10 short"    'printf "hi" | { read -n 10 x; echo "rc=$? [$x]"; }'         # rc1 [hi]
check "n3 leftover"  'printf "abcdef\n" | { read -n 3 x; read y; echo "[$x][$y]"; }' # [abc][def]
check "n3 two vars"  'printf "a b c d" | { read -n 3 x y; echo "[$x][$y]"; }'     # [a][b]
check "N3 utf8"      'printf "h\xc3\xa9llo" | { read -N 3 x; echo "[$x]"; }'      # [hél]
check "n3 utf8"      'printf "h\xc3\xa9llo" | { read -n 3 x; echo "[$x]"; }'      # [hél]
check "rn3 backslash" 'printf "a\\\\bc" | { read -rn 3 x; echo "[$x]"; }'         # [a\b]
check_err "bad-n"    'printf "x\n" | { read -n abc y; echo "rc=$?"; }'             # rc1 + "read: abc: invalid number"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
