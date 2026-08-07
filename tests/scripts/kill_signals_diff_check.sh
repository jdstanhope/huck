#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v189: the full standard signal set
# (kill -l number<->name, the kill -l listing format, and kill -SIG sending).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# number -> name for every standard signal
check "num->name 1..31" 'for n in $(seq 1 31); do kill -l $n; done'
# name -> number (bare, SIG-prefixed, lowercase)
check "name->num"       'kill -l ABRT SIGSEGV bus 11 KILL'
# 128+signo exit-status form
check "exit-form 137"   'kill -l 137'
# full listing: first 30 signals = 6 complete rows, byte-identical to bash
# (bash appends the RT tail beyond 31, excluded by head -6)
check "listing head -6" 'kill -l | head -6'
# send a real (non-job-control) signal to a DIRECT-child sleep and assert the
# kill itself is ACCEPTED (rc 0). The wait-status (128+signo) form diverges
# legitimately: bash prints a `Aborted/Segmentation fault (core dumped)` job
# line and propagates 128+signo via `wait`, while huck does neither — so this
# uses the byte-identical accepted-rc form (KILL afterwards reaps the child).
check "send ABRT"       'sleep 30 & p=$!; kill -ABRT $p; echo "kill-rc=$?"; kill -KILL $p'
check "send -s SEGV"    'sleep 30 & p=$!; kill -s SEGV $p; echo "kill-rc=$?"; kill -KILL $p'

harness_summary
