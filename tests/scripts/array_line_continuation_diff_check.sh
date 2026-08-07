#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v192: `name=\<NL>(array)` — a line
# continuation between `=`/`+=` and the array `(`.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# the byobu shape: \<NL> between `=` and `(`
check "elem index"    $'arr=\\\n(a b c)\nprintf "%s\\n" "${arr[1]}"'
check "all elems"     $'arr=\\\n(a b c)\nprintf "%s\\n" "${arr[@]}"'
check "count"         $'arr=\\\n(a b c)\necho "${#arr[@]}"'
# append form
check "append"        $'arr=(a); arr+=\\\n(b c)\necho "${arr[2]}"'
# stacked continuations
check "stacked"       $'arr=\\\n\\\n(x y)\necho "${arr[0]}"'
# negative: scalar with continuation (already worked) stays scalar
check "scalar cont"   $'v=\\\nfoo\necho "[$v]"'
# negative: a literal backslash-escape is NOT a continuation
check "escape"        $'v=\\x\necho "[$v]"'
# control: a normal inline array
check "inline array"  'arr=(p q r); echo "${arr[2]}"'

harness_summary
