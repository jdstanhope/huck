#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #410: the column layout of a job
# listing line — `[N]<flag>` + TWO spaces + the state in a 24-wide field + the
# command immediately after.
#
# Only RUNNING jobs are compared. huck does not yet list or notify Done /
# Terminated jobs at all, and renders a SIGSTOP stop as "Stopped (signal 19)"
# where bash says "Stopped" — both filed separately. Those rows would be
# testing the missing behavior, not the columns.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# `jobs -l` prints the leader pid, which differs between the two shells (and
# between runs): replace just that field with a fixed token so the COLUMNS
# after it still line up byte-for-byte.
norm() { sed -E 's|^(\[[0-9]+\][-+ ]) [0-9]+ | \1 PID |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label" "$b" "$h"
}
K='kill -KILL %1 %2 %3 2>/dev/null; wait 2>/dev/null'

# --- short form: one, two and three jobs (the +, - and blank flags) ---------
check "one running job"    "set -m; sleep 3 & jobs; $K"
check "two running jobs"   "set -m; sleep 3 & sleep 3 & jobs; $K"
check "three running jobs" "set -m; sleep 3 & sleep 3 & sleep 3 & jobs; $K"
# A longer command still starts in the same column.
check "longer command"     "set -m; sleep 3.5 & jobs; $K"

# --- long form: the pid takes the second column, state field is unchanged ---
check "jobs -l one"        "set -m; sleep 3 & jobs -l; $K"
check "jobs -l two"        "set -m; sleep 3 & sleep 3 & jobs -l; $K"

# --- #426: one line per OPERAND, in the order given -------------------------
check "operand order"      "set -m; sleep 3 & sleep 3 & jobs %2 %1; $K"
check "same operand twice" "set -m; sleep 3 & jobs %1 %1; $K"
check "operand order -r"   "set -m; sleep 3 & sleep 3 & jobs -r %2 %1; $K"
check "operand order -l"   "set -m; sleep 3 & sleep 3 & jobs -l %2 %1; $K"
check "-p operand count"   "set -m; sleep 3 & sleep 3 & jobs -p %2 %1 | wc -l; $K"
check "no operands"        "set -m; sleep 3 & sleep 3 & jobs; $K"

# --- #425: fg writes the resumed command to STDOUT --------------------------
check "fg stdout"          "set -m; sleep 0.3 & fg %1 > /dev/null; echo rc=\$?"
check "fg not stderr"      "set -m; sleep 0.3 & fg %1 2> /dev/null; echo rc=\$?"

# --- selectors that share the formatter -------------------------------------
check "jobs %1"            "set -m; sleep 3 & sleep 3 & jobs %1; $K"
check "jobs -r"            "set -m; sleep 3 & jobs -r; $K"
# -p prints bare pids, so only its shape is compared (pids differ).
check "jobs -p count"      "set -m; sleep 3 & jobs -p | wc -l; $K"

harness_summary
