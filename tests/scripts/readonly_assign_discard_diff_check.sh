#!/usr/bin/env bash
# v313 (#31): a standalone readonly-variable ASSIGNMENT error DISCARDS the
# current top-level command (bash jump_to_top_level(DISCARD)) — rc 1, unwinds
# out of loops/functions, but does NOT exit the shell (a later script line runs).
# Same DISCARD flavor as #3 (arith). huck used to print the error and CONTINUE
# (rc 0). --posix mode EXITS (127, v226). Inline-prefix / unset / for-var stay
# non-fatal.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# The readonly message already matches bash; normalize only each shell's own
# prefix (`bash: line N:` / `<huckpath>: line N:`) so the rest compares raw.
norm() { sed -E "s#^(bash|.*/huck): line [0-9]+: #SH: #"; }
check() {
  local label=$1 frag=$2 b h br hr
  b=$(bash -c "$frag" 2>&1 | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK" -c "$frag" 2>&1 | norm); hr=${PIPESTATUS[0]}
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_script() {
  local label=$1; shift; local f; f=$(mktemp); printf '%s\n' "$@" > "$f"
  local b h br hr
  b=$(bash "$f" 2>&1 | sed -E "s#^.*/[^:]+: line [0-9]+: #SH: #"); br=${PIPESTATUS[0]}
  h=$("$HUCK" "$f" 2>&1 | sed -E "s#^.*/[^:]+: line [0-9]+: #SH: #"); hr=${PIPESTATUS[0]}
  rm -f "$f"
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}

# --- The fix (red->green): standalone readonly assignment discards.
check 'readonly-assign'   'readonly r=1; r=2; echo done'
check 'uid'               'UID=5; echo done'
check 'bash-versinfo'     'BASH_VERSINFO[0]=9; echo done'
check 'assign-list'       'readonly r=1; a=1 r=2 b=3; echo x'
check 'before-after'      'echo B; r=1; readonly r; r=2; echo A'
check 'loop-unwind'       'readonly r=1; for i in 1 2 3; do echo i$i; r=2; echo t$i; done; echo END'
check 'func-unwind'       'readonly r=1; f(){ echo in; r=2; echo after_in; }; f; echo AF'

# --- Multi-line SCRIPT: discard must NOT exit the shell (later lines run).
check_script 'script-continues' 'readonly r=1' 'r=2' 'echo L2' 'echo L3'

# --- Control: a normal successful assignment must not be affected.
check 'good-assign'       'x=1; echo $x done'
# --- Readonly-diagnostic wording on the non-assignment write paths. These match
# bash as of v319 (#204 fixed there, as a side effect of routing restricted-mode
# variable protection through the ordinary readonly machinery).
check 'unset-readonly'    'readonly r=1; unset r; echo done'
check 'declare-readonly'  'readonly r=1; declare r=2; echo done'
# NOTE: two adjacent readonly cases are DELIBERATELY not tested here — they are
# PRE-EXISTING divergences in OTHER code paths (not run_assignment_list / #31),
# each filed separately: inline-prefix `r=2 cmd` skips the command bash runs
# (#203); a for-loop var readonly bind double-prints the diagnostic (#205).
# Adding them as huck-vs-bash controls would red this harness for reasons
# unrelated to #31.

if [ $FAIL -ne 0 ]; then echo "readonly_assign_discard_diff_check FAILED" >&2; exit 1; fi
echo "readonly_assign_discard_diff_check OK"
