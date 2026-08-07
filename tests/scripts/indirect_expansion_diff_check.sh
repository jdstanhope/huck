#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v95: ${!var} indirect expansion +
# [[ ]] empty-integer comparison (Tasks 1-2). Success-output fragments only;
# error-path cases (e.g. unset indirect source) are covered by integration
# tests because shell error-message prefixes never byte-match.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# ${!var} indirect expansion
check "indirect named"          'x=hi; ref=x; echo "${!ref}"'
check "indirect positional"     'set -- a b c; OPTIND=2; echo "${!OPTIND}"'
check "indirect pos default"    'set -- a b c; echo "${!2-na}"'
check "indirect default unset"  'ref=missing; echo "${!ref-fallback}"'
check "indirect default set"    'x=val; ref=x; echo "${!ref-fallback}"'
check "indirect effname unset"  'ref=missingvar; echo "[${!ref}]"'
check "indirect array element"  'a=(one two three); ref="a[1]"; echo "${!ref}"'
# ${!name[@]} / ${!name[*]} array-keys forms must still resolve (regression)
check "array keys regress"      'a=(p q r); echo "${!a[@]}"'
check "array keys star"         'a=(p q r); echo "${!a[*]}"'
# [[ ]] empty-integer comparison (empty operand treated as 0)
check "dbracket empty ge"       '[[ "" -ge 0 ]]; echo $?'
check "dbracket empty eq"       '[[ "" -eq 0 ]]; echo $?'
check "dbracket rhs empty"      '[[ 3 -gt "" ]]; echo $?'
check "dbracket both set"       'x=5; [[ $x -ge 5 ]]; echo $?'
harness_summary
