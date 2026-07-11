#!/usr/bin/env bash
# v282: byte-identical bash<->huck for exported arrays.
#   #82 — `export a=(...)` assigns the indexed array + marks it exported
#         (declare -ax), rc 0 (huck used to error "cannot export arrays").
#   #28 — an exported array is NOT inherited by a child process (bash puts no
#         array in the environment); an exported scalar IS. `printenv` is an
#         ordinary external child, so the same fragment runs under both shells.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "export array assigns"     'export a=(1 2 3); declare -p a'
check "export array rc"          'export a=(1 2 3); echo "rc=$?"'
check "export existing array"    'a=(x y); export a; declare -p a'
check "export array append"      'a=(1 2 3); export a+=(4 5); declare -p a'
check "array not in child env"   'export a=(x y z); printenv a; echo "rc=$?"'
check "scalar IS in child env"   'export s=hi; printenv s; echo "rc=$?"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
