#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v101: subshell inside command substitution.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check "subshell"       'echo "$( (echo a) )"'
check "subshell ||"    'echo "$( (echo a) || echo b )"'
check "subshell pipe"  'echo "$(echo a | (cat))"'
check "subshell semis" 'echo "$( (exit 3); echo done )"'
check "nested arith"   'echo "$( echo $((1 + 2)) )"'
check "in default"     'echo "${x:-$( (echo d) )}"'
check "in array lit"   'a=( "$( (echo x) )" ); echo "${a[0]}"'
check "plain regress"  'echo "$(echo a)"'
check "nested regress" 'echo "$(echo "$(echo b)")"'
check "backtick sub"   'echo "`(echo a)`"'

harness_summary
