#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v229: io::Error text (no `(os error N)`
# suffix) + bash prologue on the file-IO error sites (cd, redirect-open, source).
# File mode on the SAME temp path for both shells so the prologue matches.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ioe.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

checkf "cd missing"          'cd /no/such_xyz'
checkf "cd into file"        'cd /etc/hostname'
checkf "redir read missing"  'cat < /no/such_xyz'
checkf "redir write to dir"  'echo hi > /etc'
checkf "source not found"    '. /no/such_xyz'
checkf "source a directory"  '. /etc'
checkf "source a binary"     '. /bin/true'

# The source-unreadable (permission) case can't be reproduced as root (root
# bypasses mode 000), so gate it on a non-root uid.
if [[ "$(id -u)" -ne 0 ]]; then
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ioe-np.XXXXXX"); chmod 000 "$tmp"
    checkf "source unreadable" ". $tmp"
    rm -f "$tmp"
fi

harness_summary
