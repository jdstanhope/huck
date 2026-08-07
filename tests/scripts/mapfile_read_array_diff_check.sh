#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v140: read -a + mapfile/readarray.
# Each fragment runs via `-c` with a here-string (so read/mapfile stay in the
# main shell — a pipe would subshell both identically). stdout + rc compared.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
check "read -a basic"     'read -a arr <<< "a b c"; echo "${arr[*]}|${#arr[@]}"'
check "read -a IFS"       'IFS=: read -a arr <<< "a:b:c"; echo "${arr[*]}|${#arr[@]}"'
check "read -a clears"    'arr=(old x y z); read -a arr <<< "a b"; echo "${arr[*]}|${#arr[@]}"'
check "read -ra raw"      'read -ra arr <<< '"'"'x\ty'"'"'; echo "${#arr[@]}|${arr[0]}"'
check "mapfile -t"        'mapfile -t arr <<< $'"'"'x\ny\nz'"'"'; echo "${#arr[@]}|${arr[1]}"'
check "mapfile keeps nl"  'mapfile arr <<< $'"'"'a\nb'"'"'; printf "%q %q\n" "${arr[0]}" "${arr[1]}"'
check "mapfile -n"        'mapfile -n 2 -t arr <<< $'"'"'a\nb\nc\nd'"'"'; echo "${arr[*]}|${#arr[@]}"'
check "mapfile -s"        'mapfile -s 1 -t arr <<< $'"'"'a\nb\nc'"'"'; echo "${arr[*]}"'
check "mapfile -d"        'mapfile -d : -t arr <<< "a:b:c"; echo "${#arr[@]}|${arr[1]}"'
check "mapfile -O"        'mapfile -O 2 -t arr <<< $'"'"'x\ny'"'"'; echo "${!arr[*]}|${arr[*]}"'
check "readarray synonym" 'readarray -t arr <<< $'"'"'p\nq'"'"'; echo "${arr[*]}"'
check "mapfile default"   'mapfile -t <<< $'"'"'a\nb'"'"'; echo "${MAPFILE[*]}"'
harness_summary
