#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v145: export -p / export -n.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
check "p lists declare -x"  'export ZA=1 ZB=two; export -p | grep -E "declare -x Z[AB]="'
check "bare export format"  'export ZC=hi; export | grep "declare -x ZC="'
check "p readonly export"   'export ZR=1; readonly ZR; export -p | grep "ZR="'
check "n unexport keeps"    'export ZD=keep; export -n ZD; declare -p ZD'
check "n assign+unexport"   'export ZE=1; export -n ZE=2; declare -p ZE'
check "n readonly keeps"    'export ZF=1; readonly ZF; export -n ZF; declare -p ZF'
check "n unset noop"        'export -n ZNOPE; echo "rc=$?"'
check "pn unexports"        'export ZG=1; export -pn ZG; declare -p ZG'
check "p operand exports"   'ZH=1; export -p ZH; declare -p ZH | grep -o "declare -x ZH"'
check "invalid flag rc2"    'export -z 2>/dev/null; echo "rc=$?"'
harness_summary
