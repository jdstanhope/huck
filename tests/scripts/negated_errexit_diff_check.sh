#!/usr/bin/env bash
# v311 (#1): a `!`-negated pipeline must suppress `set -e`/ERR for its WHOLE
# body, including inner executions that are not their own boundary (`eval`,
# brace groups). huck exited where bash negates-and-continues, because the
# inner failing command returned Exit via the errexit gate and bypassed the
# outer `!`. Fixed by raising err_suppressed_depth around the negated body.
#
# INVARIANTS the fix must NOT break: a real `exit` inside the body still exits
# (`! eval 'exit 5'` -> rc 5), and errexit still fires normally WITHOUT `!`.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
check() {
  local label=$1 frag=$2
  local bo be br ho he hr
  bo=$(bash -c "$frag" 2>/tmp/v311_be); br=$?; be=$(cat /tmp/v311_be)
  ho=$("$HUCK" -c "$frag" 2>/tmp/v311_he); hr=$?; he=$(cat /tmp/v311_he)
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash: out=[$bo] err=[$be] rc=$br"; echo "  huck: out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- The fix (red -> green): inner failure under `!` must be suppressed.
check 'bang-eval-false'      'set -e; ! eval false; echo after'
check 'bang-eval-exit5-neg'  'set -e; ! eval "(exit 5)"; echo after'
check 'bang-brace-false'     'set -e; ! { false; }; echo after'
check 'bang-brace-false-true' 'set -e; ! { false; true; }; echo after'
check 'bang-brace-true-false' 'set -e; ! { true; false; }; echo after'
check 'bang-eval-false-true'  'set -e; ! eval "false; true"; echo after'

# --- Controls: already-correct, must stay green.
check 'bang-false'           'set -e; ! false; echo after'
check 'bang-subshell'        'set -e; ! ( false ); echo after'
check 'bang-builtin-false'   'set -e; ! builtin false; echo after'
check 'no-bang-eval-false'   'set -e; eval false; echo after'    # must STILL exit rc 1

# --- Invariant guards.
check 'bang-eval-real-exit'  'set -e; ! eval "exit 5"; echo after'         # real exit -> rc 5, NOT suppressed
check 'err-trap-bang'        'set -e; trap "echo ERR" ERR; ! eval false; echo after'   # ERR suppressed under !
check 'err-trap-no-bang'     'set -e; trap "echo ERR" ERR; eval false; echo after'     # ERR fires without !

rm -f /tmp/v311_be /tmp/v311_he
if [ $FAIL -ne 0 ]; then echo "negated_errexit_diff_check FAILED" >&2; exit 1; fi
echo "negated_errexit_diff_check OK"
