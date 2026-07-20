#!/usr/bin/env bash
# v318 (#218): process-substitution $! + assignment-RHS fd lifetime.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck): #SH: #"; }
check() { local l=$1 f=$2 b h br hr
  b=$(bash -c "$f" 2>&1); br=$?; b=$(printf '%s' "$b" | norm)
  h=$("$HUCK" -c "$f" 2>&1); hr=$?; h=$(printf '%s' "$h" | norm)
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then echo "FAIL [$l]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1; else echo "PASS [$l]"; fi; }
# --- Fix 1: $! from a process substitution
check 'bang-wait-status'  'cat <(exit 123) >/dev/null; wait "$!"; echo $?'
check 'bang-is-set'       'cat <(:) >/dev/null; [ -n "$!" ] && echo set || echo unset'
# --- control: $! from a real background job still works (last-writer-wins)
check 'bang-real-bg'      'cat <(:) >/dev/null; sleep 0 & p=$!; wait "$p"; echo "$?"'
if [ $FAIL -ne 0 ]; then echo "procsub_lifetime_diff_check FAILED" >&2; exit 1; fi
echo "procsub_lifetime_diff_check OK"
