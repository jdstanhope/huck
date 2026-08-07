#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v230 umask. File mode on the SAME temp
# path for both shells so the error prologue (`<src>: line N: umask: …`) matches.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-umask.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

checkf "octal print"      'umask 022; umask'
checkf "symbolic print"   'umask 022; umask -S'
checkf "posix print"      'umask 022; umask -p'
checkf "posix symbolic"   'umask 002; umask -p -S'
checkf "set symbolic"     'umask -S u=rwx,g=rwx,o=rx; umask'
checkf "octal range err"  'umask 09'
checkf "sym char err"     'umask g=u'
checkf "sym op err"       'umask u:rwx'
checkf "sym colon char"   'umask -S u=rwx:g=rwx,o=rx'
checkf "invalid option"   'umask -i'

harness_summary
