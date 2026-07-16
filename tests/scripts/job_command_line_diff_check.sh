#!/usr/bin/env bash
# #80: `jobs` shows the full, normalized command line (not the leading name /
# a `background job` placeholder), matching bash 5.2.21's re-rendered job text.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first (cargo build -p huck)" >&2; exit 1; }
FAIL=0
# Compare only the command column: strip `[N]<flag>`, the state word, and the
# padding, leaving `<command> &`. Both shells pad differently; we compare the
# trimmed `jobs` command text.
cmdcol() { sed -E 's/^\[[0-9]+\][-+ ] +(Running|Done|Stopped)[^ ]* +//'; }
check() {
  local label=$1 frag=$2 b h
  b=$(bash    -c "$frag" 2>/dev/null | cmdcol)
  h=$("$HUCK" -c "$frag" 2>/dev/null | cmdcol)
  if [ "$b" != "$h" ]; then echo "FAIL [$label] bash=[$b] huck=[$h]"; FAIL=1; else echo "PASS [$label]"; fi
}
# Use `sleep 0.3` so the job is still Running when `jobs` reads it, then let it finish.
check simple    'sleep 0.3 aa bb & jobs; wait'
check spaced    'sleep   0.3    aa & jobs; wait'
check pipeline  'sleep 0.3 | cat & jobs; wait'
check andor     'sleep 0.3 && echo hi & jobs; wait'
check redirect  'sleep 0.3 >/dev/null & jobs; wait'
check quoted    'sleep 0.3 "a b" & jobs; wait'
check unexpand  'x=0.3; sleep $x & jobs; wait'
if [ $FAIL -ne 0 ]; then echo "job_command_line_diff_check FAILED" >&2; exit 1; fi
echo "job_command_line_diff_check OK"
