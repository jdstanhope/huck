#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v133: a captured pipeline larger than the
# pipe buffer must not deadlock (M-119). Each fragment is wrapped in `timeout` so a
# regression shows as a FAIL (non-zero exit / truncated output), not a hung harness.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 15 bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$(timeout 15 "$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
check "large captured pipe"   'x=$(seq 1 500000 | cat); echo ${#x}'
check "three-stage captured"  'x=$(seq 1 200000 | cat | cat); echo ${#x}'
check "small captured pipe"   'x=$(seq 1 1000 | cat); echo ${#x}'
check "large producer small"  'x=$(seq 1 500000 | wc -l); echo "[$x]"'
check "pipe tr filter large"  'x=$(seq 1 500000 | tr -d "\n" | wc -c); echo "[$x]"'
check "non-capture pipe"      'seq 1 100 | wc -l'
harness_summary
