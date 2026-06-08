#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v117: array-literal element
# field-expansion (M-112) — split/cmdsub/glob/${arr[@]}/empties/mixed-index/
# append. Fragments run as file-arg scripts (L-27: huck history-expands piped
# stdin; the true non-interactive path is a file arg).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "scalar split"        's="a b c"; arr=($s); echo "${#arr[@]}"'
check "cmdsub split"        'arr=($(echo x y z)); echo "${#arr[@]}"'
check "quoted [@] fan-out"  'w=(a b c); arr=("${w[@]}"); echo "${#arr[@]}"'
check "unquoted [@] fan"    'w=(a b c); arr=(${w[@]}); echo "${#arr[@]}"'
check "[@] keeps empty"     'w=(a "" c); arr=("${w[@]}"); echo "${#arr[@]}"'
check "quoted [*] joins"    'w=(a b c); arr=("${w[*]}"); echo "${#arr[@]}"'
check "unquoted-empty drop" 'e=; arr=(a $e b); echo "${#arr[@]}"'
check "quoted-empty kept"   'e=; arr=(a "$e" b); echo "${#arr[@]}"'
check "subscript no-split"  's="a b c"; arr=([0]=$s); echo "${#arr[@]} [${arr[0]}]"'
check "mixed index cont"    's="x y"; arr=(a $s [9]=z b); echo "${!arr[@]}"'
check "append split"        'arr=(a); s="b c"; arr+=($s); echo "${#arr[@]}"'
check "append continues"    'arr=(a b); arr+=(c d); echo "${!arr[@]}"'
check "glob match"          'd=$(mktemp -d); touch "$d"/f1.txt "$d"/f2.txt; cd "$d"; arr=(*.txt); echo "${#arr[@]}"; rm -rf "$d"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
