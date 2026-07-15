#!/usr/bin/env bash
# v300 (#160): `wait -f` must be accepted (bash: "wait for full termination";
# huck's wait already blocks to termination, so accept-and-conform). The usage
# string also gains -f. Compares stdout+stderr+rc byte-identically.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
check() {
  local label=$1 frag=$2 b h
  b=$( { timeout 10 bash    -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { timeout 10 "$HUCK" -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# -f accepted, waits for the bg job to finish (rc 0)
check 'wait-f'        'sleep 0.2 & wait -f %1; echo done'
# usage string now includes -f (a bad flag prints the updated usage)
check 'wait-badflag'  'wait -Z'
# regression: -n -p var still works
check 'wait-n-p'      'sleep 0.1 & wait -n -p WP; echo "rc=$? set=${WP:+yes}"'

if [ $FAIL -ne 0 ]; then echo "wait_f_diff_check FAILED" >&2; exit 1; fi
echo "wait_f_diff_check OK"
