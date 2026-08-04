#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #418/#420: asynchronous job-status
# notices. bash announces a background job's state change whenever job control
# is on — including in a non-interactive `set -m` shell — using one of two
# forms, and prunes the job once it has been reported.
#
# NOT used here: the core-dumping signals (QUIT/ABRT/SEGV/ILL/BUS/FPE/XCPU/
# XFSZ). `core_pattern` on the CI target pipes to apport, which leaves the
# child visibly alive for seconds, so those rows are nondeterministic. The
# `(core dumped)` suffix is covered by a unit test instead.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Normalize the program name and the pid in the signal form. The LINE NUMBER is
# deliberately left alone — it is asserted.
norm() { sed -E 's|^[^:]*: line |PROG: line |; s|^(PROG: line [0-9]+: )[0-9]+ |\1PID |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- the gate: job control on, and off --------------------------------------
check "done notice"        'set -m; sleep 0.1 & sleep 1; echo MARK'
check "silent without -m"  'sleep 0.1 & sleep 1; echo MARK'
check "exit status"        'set -m; (exit 7) & sleep 1; echo MARK'
check "notice after wait"  'set -m; sleep 0.1 & wait; echo MARK'
# Reported once, then pruned: `jobs` has nothing left to say.
check "pruned after notice" 'set -m; sleep 0.1 & sleep 1; jobs; echo MID; jobs; echo END'

# --- the job-line form: bash's quiet signals --------------------------------
check "TERM"               'set -m; sleep 5 & kill -TERM %1; sleep 1; echo MARK'
check "PIPE"               'set -m; sleep 5 & kill -PIPE %1; sleep 1; echo MARK'

# --- the pid form: everything else, untrapped, non-interactive --------------
check "KILL"               'set -m; sleep 5 & kill -KILL %1; sleep 1; echo MARK'
check "HUP"                'set -m; sleep 5 & kill -HUP %1; sleep 1; echo MARK'
check "USR1"               'set -m; sleep 5 & kill -USR1 %1; sleep 1; echo MARK'
check "ALRM"               'set -m; sleep 5 & kill -ALRM %1; sleep 1; echo MARK'
# A trap on the signal flips it back to the job-line form.
check "trapped USR1"       'set -m; trap "" USR1; sleep 5 & kill -USR1 %1; sleep 1; echo MARK'
# (No SIGINT row, and no row with a NON-ignore trap: huck's background job
# leader is a forked shell that never execs, so it keeps huck's own SIGINT
# handler and the parent's trap table and absorbs signals that kill bash's
# exec'd child. That is #428's territory — a fork-time disposition problem, not
# a notification one — and it would be testing the wrong thing here.)

# --- the stop notice, and its leading blank line ----------------------------
check "STOP"               'set -m; sleep 5 & kill -STOP %1; sleep 1; echo MARK; kill -9 %1 2>/dev/null; wait 2>/dev/null'
# A stopped job is NOT pruned: it is still listed afterwards.
check "stopped stays"      'set -m; sleep 5 & kill -STOP %1; sleep 1; jobs; kill -9 %1 2>/dev/null; wait 2>/dev/null'

# --- ordering when several jobs finish in one window ------------------------
check "two jobs at once"   'set -m; sleep 0.1 & sleep 0.1 & sleep 1.2; echo MARK'
check "three jobs at once" 'set -m; sleep 0.1 & sleep 0.1 & sleep 0.1 & sleep 1.2; echo MARK'

# --- the line number in the pid form is real, not normalized ----------------
check "line number"        'set -m
sleep 5 & kill -KILL %1
sleep 0.6
echo MARK'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
