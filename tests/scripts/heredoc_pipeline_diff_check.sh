#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v134: heredoc/herestring bodies fed by a
# forked writer never deadlock (M-120). timeout-guarded so a regression FAILS.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 15 bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$(timeout 15 "$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
BIG='V=$(printf "x%.0s" $(seq 1 200000))'
check "compound large"        "$BIG"$'\n{ wc -c; } << E\n$V\nE'
check "compound awk pipe"     "$BIG"$'\n{ command awk "{print}" | wc -l; } << E\n$V\nE'
check "pipeline large"        "$BIG"$'\ncat << E | wc -c\n$V\nE'
check "captured single large" "$BIG"$'\nr=$(cat << E\n$V\nE\n); echo ${#r}'
check "herestring compound"   "$BIG"$'\n{ wc -c; } <<< "$V"'
check "small compound"        $'{ cat; } << E\nhi\nE'
check "small pipeline"        $'cat << E | wc -c\nhi\nE'
check "pipestatus heredoc"    $'false << E | true\nx\nE\necho "${PIPESTATUS[*]}"'
harness_summary
