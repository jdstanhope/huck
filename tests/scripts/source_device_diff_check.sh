#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v231 A+B: source CWD/sourcepath fallback
# + device-file/fifo acceptance. File mode on the SAME temp path for both shells.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-srcdev.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}
checkf_pipe() {
    local label="$1" body="$2" feed="$3" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-srcdev.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(printf '%s\n' "$feed" | bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$feed" | "$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

checkf       "dev null"        '. /dev/null; echo "rc=$?"'
checkf_pipe  "dev stdin"       '. /dev/stdin; echo end' 'echo PIPED'
checkf       "fifo source"     'f=$(mktemp -u "${TMPDIR:-/tmp}/huck-fifo.XXXXXX"); mkfifo "$f"; { echo "echo FIFO_OK" > "$f" & }; . "$f"; echo "rc=$?"; rm -f "$f"'
checkf       "missing"         '. /no/such_xyz_v231; echo "rc=$?"'
checkf       "directory"       '. /etc; echo "rc=$?"'
checkf       "sourcepath off"  'shopt -u sourcepath; d=$(mktemp -d "${TMPDIR:-/tmp}/huck-sd.XXXXXX"); echo "set -- m n o p" > "$d/x.sub"; cd "$d"; . x.sub; echo "$@"'

harness_summary
