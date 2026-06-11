#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v136: an array-literal / assign-prefix word
# as a command argument via eval (and declaration builtins) reconstructs + re-parses
# like bash. Does NOT test the non-eval `echo x=(a b)` case — bash syntax-errors
# there at parse time; huck reconstructs the arg (documented divergence).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "eval array"        'eval x=(a b); echo "${x[@]}|${#x[@]}"'
check "eval append"       'arr=(p); eval arr+=(x y); echo "${arr[@]}"'
check "eval idx elem"     'eval a[1]=v; echo "${a[1]}"'
check "eval idx append"   'a[2]=Q; eval a[2]+=z; echo "${a[2]}"'
check "eval quoted elem"  'eval x=([3]="a b" c); echo "${x[3]}|${x[4]}|${x[5]}"'
check "eval empty"        'eval x=(); echo "len=${#x[@]}"'
check "eval var elem"     'v=Z; eval x=($v b); echo "${x[@]}"'
check "eval var split"    'v="a b"; eval x=($v); echo "${#x[@]}|${x[@]}"'
check "escaped form"      'f(){ eval $1=\(p q\); }; f arr; echo "${arr[@]}"'
check "quoted form"       'eval "x=(a b)"; echo "${x[@]}"'
check "declare array"     'declare d=(a b); echo "${d[@]}"'
check "local array"       'f(){ local l=(a b); echo "${l[@]}"; }; f'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
