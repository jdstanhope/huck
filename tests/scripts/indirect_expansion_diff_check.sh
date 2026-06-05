#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v95: ${!var} indirect expansion +
# [[ ]] empty-integer comparison (Tasks 1-2). Success-output fragments only;
# error-path cases (e.g. unset indirect source) are covered by integration
# tests because shell error-message prefixes never byte-match.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
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
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
