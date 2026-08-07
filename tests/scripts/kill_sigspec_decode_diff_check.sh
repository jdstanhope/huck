#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #405 (pieces 1-2): one sigspec decoder
# for every position — `-SIG`, `-s SIG`, `-n SIG`, `kill -l SIG` all take a
# number OR a name — plus the `EXIT` (0) pseudo-signal.
#
# NOT covered here: the real-time signals (SIGRTMIN..SIGRTMAX, numbers 34-64).
# huck's signal table stops at 31 by design — the trap pending bitmask is an
# AtomicU32 — so `kill -l 34` and friends stay divergent until #405's third
# piece lands. The numeric SEND path already accepts them (bash hands any
# 0..64 to kill(2)), which is what the "-n 34" row below asserts.
#
# SAFETY: every send targets 12345/99999 (a pid the harness does not own) or
# uses signal 0, so nothing here can signal the harness's own process group.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
norm() { sed 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label" "$b" "$h"
}

# --- -s takes a number ------------------------------------------------------
check "-s 9"            'kill -s 9 12345; echo rc=$?'
check "-s 0"            'kill -s 0 12345; echo rc=$?'
check "-s 34"           'kill -s 34 12345; echo rc=$?'
check "-s padded number" 'kill -s " 9 " 12345; echo rc=$?'
check "-s 65 rejected"  'kill -s 65 12345; echo rc=$?'

# --- -n takes a name --------------------------------------------------------
check "-n TERM"         'kill -n TERM 12345; echo rc=$?'
check "-n SIGTERM"      'kill -n SIGTERM 12345; echo rc=$?'
check "-n term"         'kill -n term 12345; echo rc=$?'
check "-n 0"            'kill -n 0 12345; echo rc=$?'
check "-n 34"           'kill -n 34 12345; echo rc=$?'
check "-n BOGUS"        'kill -n BOGUS 12345; echo rc=$?'
check "-n 65 rejected"  'kill -n 65 12345; echo rc=$?'
check "-n -1 rejected"  'kill -n -1 12345; echo rc=$?'

# --- the EXIT pseudo-signal (0) ---------------------------------------------
check "-EXIT"           'kill -EXIT 12345; echo rc=$?'
check "-s EXIT"         'kill -s EXIT 12345; echo rc=$?'
check "-s exit"         'kill -s exit 12345; echo rc=$?'
check "-n EXIT"         'kill -n EXIT 12345; echo rc=$?'
check "-l 0 is EXIT"    'kill -l 0; echo rc=$?'
check "-l EXIT is 0"    'kill -l EXIT; echo rc=$?'
check "-l exit is 0"    'kill -l exit; echo rc=$?'
# SIGEXIT is NOT a name bash knows, and 128+0 is not the EXIT status form.
check "-l SIGEXIT bad"  'kill -l SIGEXIT; echo rc=$?'
check "-s SIGEXIT bad"  'kill -s SIGEXIT 12345; echo rc=$?'
check "-l 128 bad"      'kill -l 128; echo rc=$?'
# EXIT is absent from the listing (which starts at 1).
check "-l listing head" 'kill -l | head -6'

# --- signal 0 really is a probe, not a send ---------------------------------
check "-s 0 own pgrp"   'kill -s 0 0; echo rc=$?'
check "-n 0 own pgrp"   'kill -n 0 0; echo rc=$?'
check "-EXIT own pgrp"  'kill -EXIT 0; echo rc=$?'

# --- unchanged forms still work ---------------------------------------------
check "-9 numeric"      'kill -9 12345; echo rc=$?'
check "-TERM name"      'kill -TERM 12345; echo rc=$?'
check "-s TERM"         'kill -s TERM 12345; echo rc=$?'
check "-l TERM"         'kill -l TERM; echo rc=$?'
check "-l 15"           'kill -l 15; echo rc=$?'
check "-l 137"          'kill -l 137; echo rc=$?'

harness_summary
