#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `[[ ]]` arithmetic comparisons.
# The integer-comparison ops (-eq/-ne/-lt/-le/-gt/-ge) arith-evaluate BOTH
# operands: a bare variable name resolves to its value, `2+3` -> 5, an
# unset/empty operand -> 0. Each fragment runs through `bash -c` and
# `huck -c`; stdout+stderr+exit must match.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check "bare name resolves"   'x=10; [[ x -eq 10 ]] && echo yes'
check "arith expr operand"   '[[ 2+3 -eq 5 ]] && echo arith'
check "nested arith still"   'n=7; [[ $((n*2)) -eq 14 ]] && echo dbl'
check "unset var is zero"    '[[ unset_var -eq 0 ]] && echo zero'
check "literals -gt -lt"     '[[ 5 -gt 3 ]] && [[ 2 -lt 4 ]] && echo cmp'
check "-ne with names"       'a=4; b=5; [[ a -ne b ]] && echo ne'
check "-le -ge"              'x=3; [[ x -le 3 ]] && [[ x -ge 3 ]] && echo bound'
check "assign in operand"    'x=2; [[ x*5 -eq 10 ]] && echo mul'
check "false comparison"     '[[ 1 -eq 2 ]] && echo t || echo f'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
