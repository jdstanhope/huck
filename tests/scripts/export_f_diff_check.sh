#!/usr/bin/env bash
# v147: export -f — byte-comparable building blocks (env-key shape + declare -fx trailer + child run).
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
check "BASH_FUNC env key"   'f(){ echo x; }; export -f f; env | grep -o "BASH_FUNC_f%%" | head -1'
check "declare -fx trailer" 'f(){ echo x; }; export -f f; export -f | grep "^declare -fx f$"'
check "not a function rc"   'export -f nope 2>/dev/null; echo "rc=$?"'
check "export -p no funcs"  'f(){ echo x; }; export -f f; export -p | grep -c "BASH_FUNC" || true'
check "child runs function" 'f(){ echo HELLO; }; export -f f; bash -c f'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
