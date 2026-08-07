#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v197 bug C: `trap … KILL`/`STOP`.
# bash does NOT reject trapping KILL/STOP — it silently accepts the command
# (rc 0, no error) and stores the disposition (the signal still performs its
# default action; the disposition just can't fire). huck used to abort with
# "trap: KILL: cannot trap", which broke batch traps like
#   trap "cleanup" EXIT HUP INT TERM KILL ...
# The `trap -p` SIG-prefix (bash "SIGKILL" vs huck "KILL") is a SEPARATE,
# universal message-format divergence — normalized away here.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##; s/\bSIG([A-Z])/\1/g'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    compare "$label" "$b" "$h"
}
check "ignore '' KILL"     "trap '' KILL; echo done"
check "ignore '' STOP"     "trap '' STOP; echo done"
check "ignore '' 9"        "trap '' 9; echo done"
check "set handler KILL"   "trap 'echo hi' KILL; echo done"
check "reset - KILL"       "trap '' KILL; trap - KILL; echo done"
check "batch with KILL"    "trap 'echo bye' EXIT INT TERM KILL ABRT; echo done"
check "trap -p KILL"       "trap '' KILL; trap -p KILL"
check "trap -p set KILL"   "trap 'echo hi' KILL; trap -p KILL"
check "trap -p STOP"       "trap '' STOP; trap -p STOP"
check "reset clears -p"    "trap '' KILL; trap - KILL; trap -p KILL; echo end"
check "bogus still errors"  "trap '' BOGUS; echo done"
harness_summary
