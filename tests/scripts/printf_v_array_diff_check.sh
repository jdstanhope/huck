#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v113: `printf -v NAME[SUBSCRIPT]` writes
# an array element (M-109). Indexed (arith subscript) + associative (string key);
# plain-name -v unchanged.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "two indexed elements"  'words=(); printf -v "words[0]" %s a; printf -v "words[1]" %s b; echo "${words[0]}/${words[1]}"'
check "arith subscript"       'j=2; printf -v "x[j+1]" %s X; declare -p x'
check "unset promotes"        'printf -v "y[2]" %s hi; declare -p y'
check "associative key"       'declare -A m; printf -v "m[key]" %s V; echo "${m[key]}"'
check "plain name"            'printf -v plain %s hello; echo "$plain"'
check "element overwrite"     'a=(p q r); printf -v "a[1]" %s Z; echo "${a[*]}"'
check "value with spaces"     'printf -v "w[0]" %s "a b c"; echo "[${w[0]}]"'
check "loop build"            'c=(one two three); w=(); for ((i=0;i<${#c[@]};i++)); do printf -v "w[i]" %s "${c[i]}"; done; echo "${w[*]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
