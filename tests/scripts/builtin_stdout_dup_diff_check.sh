#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v124 Fix B: builtins honor a `>&N`
# stdout redirect. File-arg execution (L-27). Compares stdout only (2>/dev/null
# both sides) so the huck:/bash: error-prefix divergence is irrelevant.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>/dev/null; echo "EXIT:$?")
    rm -f "$tf"
    compare "$label" "$b" "$h"
}

check "echo>&2 captured empty"  'a=$(echo Z >&2); echo "[$a]"'
check "printf>&2 captured empty" 'a=$(printf "%s\n" Z >&2); echo "[$a]"'
check "echo>&1 stays stdout"     'echo KEEP >&1'
check "func >&2 under 2>/dev/null" 'f() { >&2 printf "%s\n" MSG; }; a=$( (f 2>/dev/null) ); echo "[$a]"'
check "echo>&- discards"         'a=$(echo GONE >&-); echo "[$a]"'
check "two builtins one >&2"     'a=$( { echo A; echo B >&2; } ); echo "[$a]"'

harness_summary
