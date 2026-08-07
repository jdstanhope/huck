#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v228: command-not-found error format
# (word order + non-interactive prologue). Runs each fragment as a SCRIPT FILE
# (file mode) on the SAME temp path for both shells, so the `<path>: line N:`
# prologue matches byte-for-byte. Compares stdout+stderr+rc.
#
# Scope: only the spawn-NotFound path (a resolved-but-missing external command,
# including the quoted-empty `''` real-field case). The zero-field command-word
# cases ($empty / $empty arg / $empty >redir) are a separate deferred divergence
# (bash no-ops or promotes; huck errors) and are NOT asserted here.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-cnf.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    compare "$label" "$b" "$h"
}

checkf "missing on line 1"      'nosuch_cmd_xyz'
checkf "missing reports line"   'x=1
: ok
nosuch_cmd_xyz'
checkf "missing then continues" 'nosuch_cmd_xyz
echo after'
checkf "missing with args"      'nosuch_cmd_xyz -a b c'
checkf "quoted-empty command"   "''"

harness_summary
