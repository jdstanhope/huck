#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v93: $-forms in arithmetic (M-88).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# $-expansion inside arithmetic contexts: $#, ${#arr[@]}, ${par#word}, $(cmd),
# $1, bare idents, quote removal of expanded operands, empty-as-zero.
# NOTE: bare `arr[j]` index references (no `$`) are a separate pre-existing
# unsupported form outside M-88 scope (huck errors `unexpected character: '['`),
# so they are intentionally excluded here.
check "dollar-hash cmp"   'set -- a b; (($# == 2)) && echo yes || echo no'
check "arr-len cmp"       'a=(x y z); ((${#a[@]} == 3)) && echo yes || echo no'
check "arr-len minus"     'a=(x y z); i=1; ((i < ${#a[@]} - 1)) && echo yes || echo no'
check "param-strip arith" 'set -- -a5; echo $((${1#-a} + 2))'
check "cmdsub in arith"   'echo $(( $(echo 3) * 4 ))'
check "arith-for dollar"  'a=(x y z); for ((i=0; i<${#a[@]}; i++)); do printf %s "$i"; done; echo'
check "bare ident"        'n=5; echo $((n + 1))'
check "quote removal"     'x=5; (( x == "5" )) && echo yes || echo no'
check "empty is zero"     'e=; echo $(( e ))'
check "positional arith"  'set -- 10 20; echo $(( $1 + $2 ))'
harness_summary
