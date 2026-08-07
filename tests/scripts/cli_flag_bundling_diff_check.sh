#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #230: bundled single-letter CLI flags
# (`-rc "cmd"`, `-cr`, `-nc`, `-rn`). Only SUCCESS-path stdout is compared —
# error messages carry the program-name prefix (`bash:` vs `huck:`), a known
# artifact, so those are covered by unit tests, not here.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# Compare stdout only (2>/dev/null) plus exit status.
check() {
    local label="$1"; shift
    local b h
    b=$(bash "$@" 2>/dev/null; echo "rc=$?")
    h=$("$HUCK_BIN" "$@" 2>/dev/null; echo "rc=$?")
    compare "$label" "$b" "$h"
}

check "rc echo"       -rc 'echo hi'
check "cr echo"       -cr 'echo hi'
check "rc dollar-dash" -rc 'echo $-'
check "cr dollar-dash" -cr 'echo $-'
check "rc args"       -rc 'echo $0 $1 $2' A B C
check "nc noexec"     -nc 'echo should-not-run'   # -n => nothing runs
check "rc restricted enforced" -rc 'PATH=/x; echo after'   # assignment to PATH refused; stdout differs? both refuse
check "c alone still" -c 'echo plain'
check "r then c sep"  -r -c 'echo sep'

harness_summary
