#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v197 bug A: `shift` count handling.
#   - over-range positive count: SILENT, rc 1 (bash prints nothing)
#   - negative count: "shift: N: shift count out of range", rc 1
#   - non-numeric: "shift: X: numeric argument required", rc 1
#   - exact-count / normal shifts behave identically
# The leading prog-name prefix (bash "bash: line N: " vs huck "huck: ") is a
# separate, global message-format divergence — normalized away here so the
# message TEXT and rc are what we compare.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
norm() { sed -E 's/^(bash|huck): (line [0-9]+: )?//'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "over-range (silent)"   'set -- a b; shift 5; echo done'
check "over-range no args"    'shift 5; echo done'
check "over-range rc"         'set -- a b; shift 9; echo rc=$?'
check "negative count"        'set -- a b; shift -1; echo rc=$?'
check "non-numeric"           'set -- a b; shift abc; echo rc=$?'
check "exact count empties"   'set -- a b; shift 2; echo "[$*] rc=$?"'
check "normal shift"          'set -- a b c; shift 2; echo "$*"'
check "default shift one"     'set -- a b c; shift; echo "$*"'
check "shift zero noop"       'set -- a b; shift 0; echo "$*"'
check "plus-prefixed count"   'set -- a b c; shift +2; echo "[$*]"'
check "leading/trailing ws"   'set -- a b c; n=" 2 "; shift "$n"; echo "[$*]"'
check "overflow numeric"      'set -- a b; shift 99999999999999999999; echo done'
check "hex rejected"          'set -- a b c; shift 0x2; echo "[$*] rc=$?"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
