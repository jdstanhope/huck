#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #406: the bare `%` job spec (the
# current job, like `%%`/`%+`), the empty/unresolvable operand messages for
# `kill` and `disown`, and `kill -l`'s swallowed leading option word.
#
# `%` is read by kill/jobs/fg/bg/wait/disown, so the live-job half exercises
# every builtin that resolves a spec. Job control is enabled with `set -m` so
# the background job gets its own process group, and each fragment reaps its
# child before exiting.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# Normalize the program-name token on error lines (`bash: line N:` vs
# `/path/to/huck: line N:`) — argv[0], not behavior.
norm() { sed 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label" "$b" "$h"
}

# --- bare `%` resolves to the current job -----------------------------------
J='set -m; sleep 5 & '
check "kill -0 %"       "${J}kill -0 %; echo rc=\$?; kill -KILL %; wait 2>/dev/null"
check "kill -0 % == %%" "${J}kill -0 %%; a=\$?; kill -0 %; b=\$?; echo \"\$a \$b\"; kill -KILL %; wait 2>/dev/null"
check "wait %"          "${J}kill -KILL %+ ; wait % 2>/dev/null; echo rc=\$?"
check "disown %"        "${J}disown %; echo rc=\$?; wait 2>/dev/null"
# `jobs %` prints the job line. Runs of spaces are collapsed because huck pads
# the STATUS column differently from bash (`[1]+  Running` vs `[1]+ Running`) —
# a pre-existing `jobs` listing divergence, filed separately, that has nothing
# to do with the spec resolving. Every field is still compared.
check "jobs %"          "${J}jobs % | tr -s ' '; echo rc=\$?; kill -KILL %; wait 2>/dev/null"
# fg/bg also resolve specs through the same helper, but their surrounding
# messages diverge on their own (job-control mode, "already in background"
# wording), so they are covered by their own harnesses, not here.

# --- bare `%` with no jobs = no such job (NOT "bad job spec") ---------------
check "kill % no jobs"   'kill -0 %; echo rc=$?'
check "jobs % no jobs"   'jobs %; echo rc=$?'
# (`wait %` with no jobs prints the same line but exits 127 in bash and 1 in
# huck — a pre-existing `wait` status divergence, filed separately, so only the
# message is asserted here.)
check "wait % no jobs"   'wait % 2>&1; true'
check "disown % no jobs" 'disown %; echo rc=$?'

# --- kill: the empty operand has its own message ----------------------------
check "kill empty target"    'kill ""; echo rc=$?'
check "kill -0 empty target" 'kill -0 ""; echo rc=$?'
check "kill blank target"    'kill -0 " "; echo rc=$?'
check "kill empty among many" 'kill -0 "" 12; echo rc=$?'

# --- disown: every unresolvable operand is "no such job" --------------------
check "disown empty"     'disown ""; echo rc=$?'
check "disown word"      'disown abc; echo rc=$?'
check "disown blank"     'disown " "; echo rc=$?'
check "disown zero"      'disown 0; echo rc=$?'
check "disown live pid"  'disown 99999; echo rc=$?'

# --- kill -l swallows ONE leading option word -------------------------------
check "-l -x + operand"  'kill -l -x TERM; echo rc=$?'
check "-l -x + number"   'kill -l -x 15; echo rc=$?'
# The swallowed word leaves no operands, so `-l` lists. `head -6` because
# huck's signal table stops at 31 while bash continues into SIGRTMIN..SIGRTMAX
# (#405) — the first six rows are the part both shells agree on today.
check "-l -TERM"         'kill -l -TERM | head -6; echo rc=$?'
check "-l -3"            'kill -l -3 | head -6; echo rc=$?'
check "-l -x -3"         'kill -l -x -3; echo rc=$?'
check "-l -x -y"         'kill -l -x -y; echo rc=$?'
check "-l -- -3"         'kill -l -- -3; echo rc=$?'
check "-l operand then -x" 'kill -l TERM -x; echo rc=$?'

harness_summary
