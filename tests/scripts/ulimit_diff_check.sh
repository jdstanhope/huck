#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v230 ulimit. ENV-INDEPENDENT cases only
# (round-trips of values we set in-script, and error forms). NOT `-a` absolutes.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ulimit.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

checkf "nofile roundtrip"  'ulimit -n 64; ulimit -n'
checkf "core soft set"     'ulimit -c unlimited; ulimit -c -S -- 1000; ulimit -c'
checkf "unlimited query"   'ulimit -c unlimited; ulimit -c'
checkf "invalid number"    'ulimit -n abc'
checkf "invalid option"    'ulimit -Z'

harness_summary
