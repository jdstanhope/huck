#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v151: FUNCNAME inside function bodies.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "scalar"        'f(){ echo "$FUNCNAME"; }; f'
check "array @"       'inner(){ echo "${FUNCNAME[@]}"; }; outer(){ inner; }; outer'
check "depth"         'inner(){ echo "${#FUNCNAME[@]}"; }; outer(){ inner; }; outer'
check "caller [1]"    'inner(){ echo "${FUNCNAME[1]:-none}"; }; outer(){ inner; }; outer'
check "indices !"     'inner(){ echo "${!FUNCNAME[@]}"; }; outer(){ inner; }; outer'
check "top-level"     'echo "[${FUNCNAME:-unset}] ${#FUNCNAME[@]}"'
check "restored"      'g(){ echo "$FUNCNAME"; }; f(){ g; echo "$FUNCNAME"; }; f'
check "after return"  'f(){ :; }; f; echo "[${FUNCNAME:-unset}]"'
check "single [0]"    'f(){ echo "${FUNCNAME[0]}"; }; f'
check "star joined"   'IFS=,; inner(){ echo "${FUNCNAME[*]}"; }; outer(){ inner; }; outer'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
