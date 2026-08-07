#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v187 (M-09b): a trailing redirect on a
# function DEFINITION is applied at every call, with call-time filename
# expansion. Both forms (`name() …` / `function name …`). Cases write a temp
# file then cat it so the result is on stdout. rc 0 in bash → compare full
# stdout+exit.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    # Each shell runs against a freshly emptied $D so an append (`>>`) from the
    # bash run cannot leak into the huck run (or vice versa) via shared files.
    rm -rf "$D"; mkdir -p "$D"
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    rm -rf "$D"; mkdir -p "$D"
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
D=$(mktemp -d)

check "define+call truncate" "f() { echo \"[\$1]\"; } >$D/a; f A; f B; cat $D/a"
check "append >> two calls"  "g() { echo L; } >>$D/b; g; g; cat $D/b"
check "dup >&2 captured"      "e() { echo err; } >&2; e 2>$D/c; cat $D/c"
check "call-time filename"    "Z=$D/x; h() { echo hi; } >\"\$Z\"; Z=$D/y; h; echo \"x:\$(cat $D/x 2>/dev/null)\"; echo \"y:\$(cat $D/y)\""
check "function keyword form" "function k { echo K; } >$D/d; k; cat $D/d"
check "redir + arg redir"     "m() { echo M; } >$D/e; m >$D/f; echo \"e:\$(cat $D/e 2>/dev/null)\"; echo \"f:\$(cat $D/f)\""
check "control no redirect"   'c() { echo plain; }; c'

rm -rf "$D"
harness_summary
