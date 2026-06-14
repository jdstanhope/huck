#!/usr/bin/env bash
# Byte-identical bash<->huck harness for ARRAY SUBSCRIPTS in arithmetic:
# `$(( arr[i] ))`, `(( a[i]++ ))`, compound-assign on elements, indexed vs
# associative subscript semantics, and `let` element assignment. Each
# fragment runs through `bash -c` and `huck -c`; stdout+stderr+exit must
# match byte-for-byte.
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

# --- indexed array element reads as operands ---
check "sum literal indices"   'arr=(10 20 30); echo $(( arr[0] + arr[1] + arr[2] ))'
check "index via scalar var"  'arr=(5 6 7); i=2; echo $(( arr[i] ))'
check "index via arith expr"  'arr=(5 6 7); echo $(( arr[1+1] ))'
check "scalar plus element"   'n=5; arr=(7 8); echo $(( n + arr[0] ))'
check "unset element is zero" 'echo $(( undefined_arr[3] ))'
check "unset elem in bounds"  'arr=(1 2 3); echo $(( arr[7] ))'
check "element holding expr"  'arr=("1+1" 5); echo $(( arr[0] ))'
check "nested subscript"      'arr=(0 1 2 3); i=(2); echo $(( arr[i[0]+1] ))'

# --- indexed lvalues: post/pre inc/dec, assign, compound ---
check "post-inc element"      'a=(1 2 3); (( a[1]++ )); echo "${a[1]}"'
check "pre-inc element val"   'a=(1 2 3); echo $(( ++a[1] )); echo "${a[1]}"'
check "post-dec element"      'a=(5 5 5); (( a[2]-- )); echo "${a[2]}"'
check "compound += element"   'a=(10 20); (( a[0] += a[1] )); echo "${a[0]}"'
check "assign arith subscript" 'arr=(0 0 0); (( arr[1+1] = 9 )); echo "${arr[2]}"'
check "compound *= element"   'a=(2 3 4); (( a[2] *= 5 )); echo "${a[2]}"'
check "assign to new index"   'a=(1 2); (( a[5] = 99 )); echo "${a[5]} ${#a[@]}"'

# --- the common reduction idiom ---
check "sum-loop reduction"    'arr=(2 4 6); s=0; for i in 0 1 2; do (( s += arr[i] )); done; echo "$s"'

# --- associative arrays: subscript is a literal key ---
check "assoc read operands"   'declare -A m=([x]=5 [y]=10); echo $(( m[x] + m[y] ))'
check "assoc compound *="     'declare -A m=([k]=4); (( m[k] *= 3 )); echo "${m[k]}"'
check "assoc key word"        'declare -A m=([key1]=7 [key2]=8); echo $(( m[key1] + m[key2] ))'
check "assoc assign element"  'declare -A m=([a]=1); (( m[a] = 42 )); echo "${m[a]}"'
check "assoc unset key zero"  'declare -A m=([a]=1); echo $(( m[nope] ))'

# --- let uses the same arith path ---
check "let element assign"    'let "a[0]=99"; echo "${a[0]}"'
check "let element compound"  'a=(4 5); let "a[1]+=10"; echo "${a[1]}"'

# --- scalar arithmetic must still work (regression guard) ---
check "scalar add"            'x=5; echo $(( x + 3 ))'
check "scalar power assign"   '(( y = 2**3 )); echo "$y"'
check "scalar post-inc"       'i=0; echo $(( i++ )); echo "$i"'
check "scalar cascade assign" 'echo $(( a = b = 5 )); echo "$a $b"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
