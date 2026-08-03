#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #4: `kill` with a negative (process
# group) or zero PID target, plus the `--` end-of-options token that makes the
# `kill -- -$pgid` form reachable with the default signal.
#
# SAFETY: every fragment here signals either signal 0 (an existence probe) or a
# process group that cannot exist (-99999) or one the harness created under
# `set -m`. Never add a real-signal fragment against pid 0 / -0 / -1 — under a
# non-job-control `bash -c` those name the HARNESS's own process group.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Normalize the leading program-name token on error lines (`bash: line N:` vs
# `/path/to/huck: line N:`) — that prefix is the invoking binary's argv[0], a
# non-behavioral artifact, not a huck<->bash difference. Everything after
# `: line N:` must still match byte-for-byte.
norm() { sed 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- negative / zero targets go straight to kill(2) -------------------------
# -1: every process the caller may signal (signal 0 => pure permission probe).
check "sig0 to -1"        'kill -0 -1; echo rc=$?'
# 0 and -0 both name the caller's own process group.
check "sig0 to 0"         'kill -0 0; echo rc=$?'
check "sig0 to -0"        'kill -0 -0; echo rc=$?'
# A negative group that cannot exist: ESRCH, rendered bash-style (bare
# strerror, no Rust "(os error N)" tail).
check "sig0 to -99999"    'kill -0 -99999; echo rc=$?'
check "SIGTERM to -99999" 'kill -TERM -99999; echo rc=$?'
check "-s to -99999"      'kill -s TERM -99999; echo rc=$?'
check "-n to -99999"      'kill -n 9 -99999; echo rc=$?'
# A sigspec is only recognised as the FIRST word: a later -N is a group target.
check "later -9 is a pgrp" 'kill -0 12 -9; echo rc=$?'
# Out-of-i32-range stays a bad target, not a group.
check "overflowing target" 'kill -0 -1234567890123; echo rc=$?'
# Mixed targets: each is reported independently, worst status wins.
check "mixed targets"     'kill -0 12 abc -99999; echo rc=$?'

# --- `--` end of options ----------------------------------------------------
check "-- then -pgid"     'kill -- -99999; echo rc=$?'
check "sig then --"       'kill -0 -- -99999; echo rc=$?'
check "-s then --"        'kill -s TERM -- -99999; echo rc=$?'
check "-n then --"        'kill -n 9 -- -99999; echo rc=$?'
# Only ONE leading `--` is consumed; a second one is an ordinary bad target.
check "second -- is target" 'kill -- --; echo rc=$?'
check "-- after sig, twice" 'kill -0 -- --; echo rc=$?'
# `--` only ends options at the HEAD of the targets: after a plain target,
# option processing is already over so it is just another bad target.
check "-- mid-target-list" 'kill -0 12 -- 13; echo rc=$?'
# Everything after `--` is a target, including things that look like options.
check "-- hides -s"       'kill -- -s TERM 99999; echo rc=$?'

# --- a real process group ---------------------------------------------------
# `set -m` puts the background job in its own group, so -$! is a live pgrp:
# probe it, then signal the whole group by its negative id.
check "live pgrp probe+kill" \
    'set -m; sleep 5 & p=$!; kill -0 -$p; echo "neg-rc=$?"; kill -KILL -$p; echo "kill-rc=$?"; wait 2>/dev/null; echo done'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
