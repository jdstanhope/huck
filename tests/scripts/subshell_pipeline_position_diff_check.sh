#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v100: subshell-headed pipeline in any position (M-11a).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# A subshell `( ... )` that heads a pipeline, in every command position.
check "sub pipe ;"        'echo z; ( echo a ) | sort'
check "sub pipe &&"       "true && ( printf 'b\\na\\n' ) | sort"
check "sub pipe ||"       'false || ( echo x ) | cat'
check "brace pipe ;"      'echo z; { echo a; echo b; } | sort'
check "if pipe ;"         'echo z; if true; then echo a; fi | cat'
check "fn body sub pipe"  'f() { echo z; ( echo a ) | sort; }; f'
check "for body sub pipe" 'for i in 1 2; do ( echo $i ) | cat; done'
check "negated sub pipe"  'echo z; ! ( false ) | cat; echo $?'
check "mixed mid compound" 'echo z; ( echo a ) | { cat; } | cat'
check "negated after &&"  'true && ! ( false ) | cat; echo $?'
check "first-pos regress" '( echo a ) | sort; echo z'
check "plain seq regress" 'echo a; echo b; true && echo y'
check "nvm shape"         $'f() {\n  local X\n  ( for n in b a; do echo $n & done; wait ) | sort\n}\nf'

harness_summary
