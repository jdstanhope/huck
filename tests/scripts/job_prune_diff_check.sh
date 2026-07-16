#!/usr/bin/env bash
# #175: completed background jobs are pruned non-interactively (matching bash):
# after wait, after each command, and via `wait $!`; running/stopped kept.
#
# NOTE: these cases run under `-c` mode (`bash -c` / `huck -c`) ON PURPOSE.
# bash has two non-interactive cadences: `-c` prunes completed jobs at the
# command boundary (huck matches this), but script-file/stdin mode retains a
# completed job until the first `jobs`/`wait` reports it (a one-time `[1]+ Done`
# echo, then prune). huck prunes uniformly and does NOT reproduce that one-time
# echo — a documented by-design divergence (#179, docs/bash-divergences.md).
# Do NOT convert these cases to script-file mode; they would diverge by design.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first" >&2; exit 1; }
FAIL=0
check() { # label frag
  local b h
  b=$(bash    -c "$2" 2>/dev/null)
  h=$("$HUCK" -c "$2" 2>/dev/null)
  if [ "$b" != "$h" ]; then echo "FAIL [$1] bash=[$b] huck=[$h]"; FAIL=1; else echo "PASS [$1]"; fi
}
check bgwait_empty   'sleep 0 & wait $!; echo "n=$(jobs | wc -l)"'
check loop_bounded   'i=0; while [ $i -lt 60 ]; do sleep 0 & wait $!; i=$((i+1)); done; echo "n=$(jobs | wc -l)"'
check nowait_pruned  'sleep 0 & sleep 0.2; echo "n=$(jobs | wc -l)"'
check many_nowait    'i=0; while [ $i -lt 60 ]; do sleep 0 & i=$((i+1)); done; sleep 0.3; echo "n=$(jobs | wc -l)"'
check running_kept   'sleep 0.4 & echo "n=$(jobs | wc -l)"; wait'
# #175: pruning the visible `jobs` list must NOT drop the waitable exit status —
# bash retains it so `wait $pid` after an auto-prune still resolves (repeatedly).
check waitpid_after_prune 'sleep 0.1 & p=$!; sleep 0.3; wait $p; echo "rc=$?"'
check failed_bg_status    '(exit 7) & p=$!; sleep 0.2; wait $p; echo "rc=$?"'
check double_wait_same_rc 'sleep 0.1 & p=$!; wait $p; echo "a=$?"; wait $p; echo "b=$?"'
if [ $FAIL -ne 0 ]; then echo "job_prune_diff_check FAILED" >&2; exit 1; fi
echo "job_prune_diff_check OK"
