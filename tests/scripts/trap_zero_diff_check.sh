#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v194: `trap … 0` (numeric 0 ≡ EXIT).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
check "register 0"   "trap 'echo EX' 0; echo body"
check "0 plus sig"   "trap 'echo EX' 0 2; echo body"
check "ignore '' 0"  "trap '' 0; echo body"
check "reset - 0"    "trap 'echo EX' 0; trap - 0; echo body"
check "trap -p 0"    "trap 'echo A' 0; trap -p 0"
check "EXIT name"    "trap 'echo EX' EXIT; echo body"
harness_summary
