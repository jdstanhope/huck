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

# --- #398: a CLOSE of fd 1 must survive the REST of the redirect list. The
#     scope saves each fd it is about to replace by dup'ing it, and a plain
#     `dup(2)` takes the LOWEST free descriptor — so once `>&-` freed fd 1, the
#     NEXT redirect's save-dup landed on fd 1 itself, reopening it (onto fd 2's
#     original target) and letting the builtin's write succeed. The close only
#     evaporated when a LATER redirect followed it, which is why `echo hi >&-`
#     alone always worked and `echo hi 2>/dev/null >&-` (close LAST) did too.
#     stderr is dropped by this harness's driver, so these rows compare the
#     STDOUT that must not appear, plus the status.
check "close then 2>file"        'echo hi >&- 2>/dev/null; echo "rc=$?"'
check "close then 3>file"        'echo hi >&- 3>/dev/null; echo "rc=$?"'
check "explicit 1>&- then 2>"    'echo hi 1>&- 2>/dev/null; echo "rc=$?"'
check "close then 0<file"        'echo hi >&- 0</dev/null; echo "rc=$?"'
check "close last"               'echo hi 2>/dev/null >&-; echo "rc=$?"'
check "printf close then 2>"     'printf hi >&- 2>/dev/null; echo "rc=$?"'
check "pwd close then 2>"        'pwd >&- 2>/dev/null; echo "rc=$?"'
check "declare -p close then 2>" 'declare -p PWD >&- 2>/dev/null; echo "rc=$?"'
check "group close then 2>"      '{ echo hi >&-; } 2>/dev/null; echo "rc=$?"'
check "external close then 2>"   '/bin/echo hi >&- 2>/dev/null; echo "rc=$?"'
# The saved fd must not be observable as a live descriptor either.
check "close then probe fd1"     'echo hi >&- 2>/dev/null; echo "rc=$?"; echo still-here'
check "two closes then open"     'echo hi >&- 2>&- 3>/dev/null; echo "rc=$?"'

harness_summary
