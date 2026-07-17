#!/usr/bin/env bash
# v310 (#176): a compound group/subshell's stderr under `2>&1` INSIDE a command
# substitution must merge into the captured stdout in program order, matching
# bash. huck applied the group's `2>&1` only as a real dup2, but the comsub
# capture is an in-memory Vec (builtins) / per-command pipe (externals) with no
# single real fd — so stderr leaked out of the capture. Fixed by routing the
# inner body's stderr through a software Merged sink (see with_redirect_scope).
#
# Each case compares (stdout, stderr, exit_code) byte-identically. `printf
# "<%s>"` prints the capture so its ordering vs a leaked stream is visible.
#
# OUT OF SCOPE (#195): the `2>&1 >file` ordering case — pinned below to huck's
# CURRENT behavior so it is not silently changed.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Compare huck vs bash on (stdout | stderr | rc), byte-identical.
check() {
  local label=$1 frag=$2
  local bo be br ho he hr
  bo=$(bash -c "$frag" 2>/tmp/v310_be); br=$?; be=$(cat /tmp/v310_be)
  ho=$("$HUCK" -c "$frag" 2>/tmp/v310_he); hr=$?; he=$(cat /tmp/v310_he)
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"
    echo "  bash: out=[$bo] err=[$be] rc=$br"
    echo "  huck: out=[$ho] err=[$he] rc=$hr"
    FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- The fix: compound 2>&1 inside $() must capture both streams, in order.
check 'builtin-group'   'x=$( { echo out; echo er >&2; } 2>&1 ); printf "<%s>" "$x"'
check 'external-group'  'x=$( { /bin/echo out; /bin/echo er >&2; } 2>&1 ); printf "<%s>" "$x"'
check 'subshell'        'x=$( ( echo out; echo er >&2 ) 2>&1 ); printf "<%s>" "$x"'
check 'mixed-group'     'x=$( { echo out; /bin/echo er >&2; } 2>&1 ); printf "<%s>" "$x"'
check 'group-in-func'   'f(){ got=$( { echo out; echo er >&2; } 2>&1 ); printf "<%s>" "$got"; }; f'
check 'nested-comsub'   'x=$( echo "[$( { echo a; echo b >&2; } 2>&1 )]" ); printf "<%s>" "$x"'

# --- Controls: must stay correct (no over-fire, no regression).
check 'no-merge'        'x=$( echo out; echo er >&2 ); printf "<%s>" "$x"'      # er -> terminal
check 'simple-2>&1'     'x=$( echo hi 2>&1 ); printf "<%s>" "$x"'
check 'terminal-group'  '{ echo out; echo er >&2; } 2>&1'                        # no comsub, both -> term
check 'stdout-to-file'  'x=$( { echo out; echo er >&2; } >/tmp/v310_f 2>&1 ); printf "<%s>[%s]" "$x" "$(cat /tmp/v310_f)"'  # both -> file

# --- OUT OF SCOPE (#195): 2>&1 >file. bash captures er, huck currently leaks it.
# Pin huck's CURRENT behavior so this fix neither fixes nor further breaks it.
# (Deliberately compares huck-to-itself: a change here should be a conscious #195
# decision, surfaced by this line flipping.)
ho=$($HUCK -c 'x=$( { echo out; echo er >&2; } 2>&1 >/tmp/v310_g ); printf "cap=<%s>" "$x"' 2>/dev/null)
# NOTE (deviation, see task-1-report.md): huck's leak for this case lands on
# its real fd 1 (not fd 2), so the pin includes the leaked "er" line ahead of
# the printf output rather than "cap=<>" alone.
oos_expected='er
cap=<>'
if [ "$ho" = "$oos_expected" ]; then echo "PASS [oos-2>&1>file-pinned (#195)]"; else echo "FAIL [oos-2>&1>file changed: [$ho] — reconcile with #195]"; FAIL=1; fi

rm -f /tmp/v310_be /tmp/v310_he /tmp/v310_f /tmp/v310_g
if [ $FAIL -ne 0 ]; then echo "comsub_merge_stderr_diff_check FAILED" >&2; exit 1; fi
echo "comsub_merge_stderr_diff_check OK"
