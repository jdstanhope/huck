#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v144: brace expansion in array literals
# (pure-literal braces + braces adjacent to $-expansions).
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
# pure-literal braces
check "range"         'a=({1..3} z); declare -p a'
check "cartesian"     'a=({a,b}{1,2}); declare -p a'
check "single brace"  'a=({1} z); declare -p a'
check "literal nums"  'a=(f{1,2}.x); declare -p a'
check "quoted brace"  'a=("{1,2}" x); declare -p a'
check "sub then bare" 'a=([0]=p q{1,2} r); declare -p a'
check "append"        'a=(x); a+=({1,2}); declare -p a'
check "local array"   'f(){ local a=({1..3}); echo "${#a[@]}:${a[*]}"; }; f'
check "assoc literal"  'declare -A m=([k]=x{a,b}); echo "[${m[k]}]"'
# braces adjacent to $-expansions (the completed fix)
check "var adjacent"  'V=Q; a=(x{1,2}$V); declare -p a'
check "cmdsub adjacent" 'a=(x{1,2}$(echo Q)); declare -p a'
check "cmdsub split"  'a=(pre{1,2}$(echo m n)); declare -p a'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
