#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v151: FUNCNAME inside function bodies.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
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
harness_summary
