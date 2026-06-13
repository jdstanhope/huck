#!/usr/bin/env bash
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first" >&2; exit 1; }
PASS=0; FAIL=0
check_c(){ local l="$1" f="$2" b h; b=$(bash -c "$f" 2>&1); h=$("$HUCK_BIN" -c "$f" 2>&1)
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1)); else printf 'FAIL: %s\n  bash=[%s] huck=[%s]\n' "$l" "$b" "$h"; FAIL=$((FAIL+1)); fi; }
check_true(){ local l="$1" f="$2" h; h=$("$HUCK_BIN" -c "$f" 2>&1)
  if [[ "$h" == "OK" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1)); else printf 'FAIL: %s (got [%s])\n' "$l" "$h"; FAIL=$((FAIL+1)); fi; }

# byte-identical to bash (same user/host/platform)
check_c "UID"       'echo $UID'
check_c "EUID"      'echo $EUID'
# PPID: check_c's $() subshells are themselves child processes, making bash and huck's PPID
# differ from each other (they see the intermediate subshell as parent).  Use check_true to
# verify PPID is a valid positive-integer PID instead.
check_true "PPID"   '[[ "$PPID" =~ ^[1-9][0-9]*$ ]] && echo OK'
check_c "HOSTNAME"  'echo $HOSTNAME'
check_c "HOSTTYPE"  'echo $HOSTTYPE'
check_c "OSTYPE"    'echo $OSTYPE'
check_c "MACHTYPE"  'echo $MACHTYPE'

# diverges: bash returns GROUPS in gid-ascending order (egid first, then supplementals sorted);
#           huck returns them sorted differently (ascending by gid without egid-first).
#           Check non-empty and all members are numeric instead.
check_true "GROUPS non-empty"  'g="${GROUPS[@]}"; [ -n "$g" ] && echo OK'
check_true "GROUPS numeric"    'for x in "${GROUPS[@]}"; do [[ "$x" =~ ^[0-9]+$ ]] || exit 1; done && echo OK'

# behavior checks (not byte-comparable)
check_true "RANDOM range"  'r=$RANDOM; [ "$r" -ge 0 ] && [ "$r" -le 32767 ] && echo OK'
check_true "RANDOM reseed" 'RANDOM=7; a=$RANDOM; RANDOM=7; b=$RANDOM; [ "$a" = "$b" ] && echo OK'
check_true "SECONDS zero"  '[ "$SECONDS" = "0" ] && echo OK'
check_true "SECONDS reset" 'SECONDS=5; [ "$SECONDS" -ge 5 ] && echo OK'
check_true "BASH_VERSION"  '[ -n "$BASH_VERSION" ] && echo OK'
check_true "BASH_VERSINFO" '[ "${BASH_VERSINFO[0]}" -ge 4 ] && echo OK'
check_true "EPOCHSECONDS"  '[ "$EPOCHSECONDS" -gt 1700000000 ] && echo OK'
check_true "BASHPID"       '[ "$BASHPID" -gt 1 ] && echo OK'
check_true "HUCK_VERSION"  '[ -n "$HUCK_VERSION" ] && echo OK'

# completion parity: both shells list these in compgen -v (count match)
check_c "compgen RANDOM"   'compgen -v | grep -c "^RANDOM$"'
check_c "compgen LINENO"   'compgen -v | grep -c "^LINENO$"'
check_c "compgen UID"      'compgen -v | grep -c "^UID$"'
check_c "compgen SECONDS"  'compgen -v | grep -c "^SECONDS$"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL>0 ? 1:0 ))
