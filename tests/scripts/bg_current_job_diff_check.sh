#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #412: `bg` on a job that is already
# running (a notice, not an error), the current-job operand default, and the
# `<spec>: no such job` wording fg/bg/disown use when there is no current job.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
norm() { sed 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label" "$b" "$h"
}
# Job control on, and every fragment kills and reaps what it started.
J='set -m; '
R='; kill -KILL %1 2>/dev/null; wait 2>/dev/null'

# --- a running job is "already in background", and that is rc 0 -------------
check "bg %1 running"    "${J}sleep 2 & bg %1; echo rc=\$?${R}"
check "bg bare running"  "${J}sleep 2 & bg; echo rc=\$?${R}"
check "bg %+ running"    "${J}sleep 2 & bg %+; echo rc=\$?${R}"
check "bg % running"     "${J}sleep 2 & bg %; echo rc=\$?${R}"
# Named by BARE id, so the second job says "job 2".
check "bg %2 of two"     "${J}sleep 2 & sleep 2 & bg %2; echo rc=\$?; kill -KILL %1 %2 2>/dev/null; wait 2>/dev/null"

# --- no current job ---------------------------------------------------------
# (All of these enable job control: without `set -m` BOTH shells answer "no job
# control" before ever looking at the operand — that path is fg_bg_no_jobcontrol
# _diff_check.sh's, #518/#416.)
check "bg bare no jobs"     'set -m; bg; echo rc=$?'
check "fg bare no jobs"     'set -m; fg; echo rc=$?'
check "disown bare no jobs" 'disown; echo rc=$?'  # disown needs no job control
check "bg %1 no jobs"       'set -m; bg %1; echo rc=$?'
check "fg %1 no jobs"       'set -m; fg %1; echo rc=$?'

# (Resuming a genuinely STOPPED job is covered by job_stop_cont_diff_check.sh.
# It cannot be re-checked here: bash prints an asynchronous `[1]+  Stopped`
# notice huck does not, and filtering it needs a pipe — which puts `bg` in a
# subshell, where BOTH shells now report "no job control" (#416). The missing
# async notice is a separate, filed divergence.)

# --- #417: bg takes a LIST of specs; status 1 if any operand failed ---------
check "two specs, none exist"  "${J}bg %1 %2; echo rc=\$?"
check "one live, one missing"  "${J}sleep 2 & bg %1 %2; echo rc=\$?${R}"
check "missing then live"      "${J}sleep 2 & bg %2 %1; echo rc=\$?${R}"
check "two live jobs"          "${J}sleep 2 & sleep 2 & bg %1 %2; echo rc=\$?; kill -KILL %1 %2 2>/dev/null; wait 2>/dev/null"
check "same job twice"         "${J}sleep 2 & bg %1 %1; echo rc=\$?${R}"

# --- usage / option errors are unchanged ------------------------------------
check "bg -x"            'set -m; bg -x; echo rc=$?'

harness_summary
