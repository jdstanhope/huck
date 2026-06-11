#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v140: read -a + mapfile/readarray.
# Each fragment runs via `-c` with a here-string (so read/mapfile stay in the
# main shell — a pipe would subshell both identically). stdout + rc compared.
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
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
