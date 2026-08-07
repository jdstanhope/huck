#!/usr/bin/env bash
# v284: byte-identical bash<->huck for `history N` (#7) and -d/-w/-r/-a (#6).
# File-arg execution (L-27: huck history-expands piped stdin). HISTFILE=/dev/null
# isolates; history is populated with `history -r <fixture>` (works non-interactively).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'cmd-one\ncmd-two\ncmd-three\ncmd-four\n' > "$WORK/fix"
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
check() {
    local label="$1" frag="$2" b h
    printf '%s\n' "$frag" > "$WORK/frag.sh"
    b=$(cd "$WORK" && bash ./frag.sh 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && "$HUCK_BIN" ./frag.sh 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    compare "$label" "$b" "$h"
}
POP='HISTFILE=/dev/null; history -c; history -r fix;'
check "list all format"   "$POP history"
check "history 2"         "$POP history 2"
check "history 0"         "$POP history 0"
check "history 99"        "$POP history 99"
check "delete single"     "$POP history -d 2; history"
check "delete negative"   "$POP history -d -1; history"
check "delete range"      "$POP history -d 2-3; history"
check "delete oob err"    "$POP history -d 9; echo rc=\$?"
check "delete nonnum err" "$POP history -d abc; echo rc=\$?"
check "read append"       "$POP history -r fix; history"
check "write file"        "$POP history -w out; cat out"
check "append after read" "$POP : > ap; history -a ap; echo \"ap=[\$(cat ap)]\""
check "delete empty crash guard" "$POP history -d ''; echo rc=\$?"
check "double-dash lists all" "$POP history --"
check "nonnumeric count arg"    "$POP history abc; echo rc=\$?"
check "nonnumeric-first multi"  "$POP history abc def; echo rc=\$?"
# NB: bash's "too many arguments" error for the trailing-count path
# discards the REST OF THE SAME PARSED LIST (a bash-internal
# jump-to-top-level(DISCARD) quirk specific to this one check -- e.g.
# `history 2 3 || echo x` never runs the `|| echo x` either). That list-abort
# propagation is out of scope for the history builtin fix; put the trailing
# `echo rc=$?` on its own line so both shells resume at the next top-level
# command and we verify just the message/rc this fix is responsible for.
check "too many arguments" "$(printf '%s\nhistory 2 3\necho rc=$?' "$POP")"
harness_summary
