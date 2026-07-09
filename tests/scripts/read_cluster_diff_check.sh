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

# --- B-03 (last-field trailing NON-ws IFS delimiter) was REVERTED in v276:
# no simple heuristic matches bash's read.def last-field splitter across the
# ifs-posix multi-char-IFS cases. Deferred to its own iteration. The whole-line
# ws-IFS trimming below is unchanged and still matches bash. ---
check "b03 a:b:c:d 3v" 'printf "a:b:c:d\n"| { IFS=: read x y z; echo "[$x][$y][$z]"; }'  # ws-only trim, matches bash
check "b03 single var" 'printf "a:b:\n"   | { IFS=: read x; echo "[$x]"; }'              # single-name path unchanged
check "b03 ws trail"   'printf "a b  \n"  | { read x y; echo "[$x][$y]"; }'              # default ws-IFS

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

# --- Final-review Finding 1: -N assigns RAW (no IFS split/trim), -n still splits ---
check "N5 raw 2v"     'printf "a b c" | { read -N 5 x y; echo "rc=$? [$x][$y]"; }'      # rc0 [a b c][]
check "N5 raw trim"   'printf "  a  " | { read -N 5 x; echo "rc=$? [$x]"; }'            # rc0 [  a  ]
check "N3 raw array"  'printf "a b" | { read -N 3 -a arr; echo "rc=$? n=${#arr[@]} [${arr[0]}]"; }' # rc0 n=1 [a b]
check "N9 raw eof"    'printf "a b" | { read -N 9 x y; echo "rc=$? [$x][$y]"; }'        # rc1 [a b][]

# --- M-163: -t TIMEOUT timed reads ---
check "t-data"        'printf "line\n" | { read -t 5 x; echo "rc=$? [$x]"; }'    # rc0 [line]
check "t0-file-ready" 'read -t 0 x < /etc/hostname; echo "rc=$?"'                # rc0 (regular file always ready)
check_err "bad-t"     'printf "x\n" | { read -t abc y; echo "rc=$?"; }'          # rc1 + "read: abc: invalid timeout specification"
check "t-frac-data"   'printf "z\n" | { read -t 0.5 x; echo "rc=$? [$x]"; }'     # rc0 [z]

# Timeout-expiry cases run a slow producer; `check`'s stdin-piped-script
# form can't express that cleanly, so these compare via `bash -c`/`huck -c`
# directly (rc-only — huck's error stream on SIGALRM-style timeout doesn't
# need byte-identical matching, just the exit code + any partial data).
check_rc_only() { local l="$1" f="$2" b h
  b=$(bash -c "$f" 2>/dev/null; echo "EXIT:$?")
  h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

check_rc_only "t-timeout"   '( sleep 2; echo late ) | { read -t 1 x; echo "rc=$? [$x]"; }'   # rc142 []
check_rc_only "t-partial"   '( printf par; sleep 2 ) | { read -t 1 x; echo "rc=$? [$x]"; }'   # rc142 [par]
check_rc_only "t-frac-to"   '( sleep 1; echo l ) | { read -t 0.3 x; echo "rc=$?"; }'          # rc142

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
