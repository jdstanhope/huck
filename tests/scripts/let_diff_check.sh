#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `let` arithmetic builtin.
# Each argument is one arithmetic expression, evaluated left-to-right with
# side effects applied; exit status is 0 if the LAST value is non-zero, 1 if
# it is zero (like `(( ))`). Each fragment runs through `bash -c` and
# `huck -c`; stdout+exit must match. (Stderr wording for the no-arg case
# diverges, so that case asserts exit only — see "no args" below.)
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check "quoted spaced expr"   'let "x = 5**2"; echo "$x"'
check "multi-arg side fx"    "let a=3 b=4 c='a+b'; echo \"\$a \$b \$c\""
check "nonzero result rc0"   'let "1+1"; echo "rc=$?"'
check "zero result rc1"      'let "5-5"; echo "rc=$?"'
check "comma seq last wins"  'let "n=10, n>5"; echo "$n rc=$?"'
check "unquoted no-space"    'let x=3; echo "$x"'
check "no args exits 1"      'let; echo "rc=$?"'
check "negative result rc0"  'let "v = -4"; echo "$v rc=$?"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
