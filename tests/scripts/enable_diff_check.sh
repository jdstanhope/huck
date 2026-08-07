#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v230 enable. File mode, same temp path
# both shells (so the `not a shell builtin` prologue matches).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-enable.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

checkf "list special"     'enable -ps'
checkf "list all special" 'enable -aps'
checkf "list disabled"    'enable -nps'
checkf "disable type"     'enable -n test; type -t test'
checkf "reenable type"    'enable -n test; enable test; type -t test'
checkf "unknown builtin"  'enable sh bash'

harness_summary
