#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v177: `$((` disambiguation. A command
# substitution whose body starts with a subshell, written glued as `$((`, must
# parse as command substitution (not arithmetic) and match bash; real arithmetic
# expansions must be unaffected. Each case EXECUTES and asserts identical
# stdout+exit.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the bug: glued $(( subshell ) ... ) is a command substitution ---
check "subshell + 2>&1"        'echo $((echo hi) 2>&1)'
check "subshell piped"         'echo $((echo a) | tr a-z A-Z)'
check "subshell multi-cmd"     'echo $((printf X; printf Y) 2>/dev/null)'
check "subshell redirect capt" 'v=$( (printf P; printf Q) 2>/dev/null ); echo "[$v]"'
check "glued capture"          'v=$((printf m; printf n) 2>/dev/null); echo "[$v]"'

# --- regressions: real arithmetic, unaffected ---
check "plain arith"            'echo $((1+2))'
check "arith paren subexpr"    'echo $(( (1+2)*3 ))'
check "arith double paren"     'echo $(( ((4)) ))'
check "arith ternary"          'echo $((1>0?2:3))'
check "spaced subshell form"   'echo $( (echo s) 2>&1 )'

harness_summary
