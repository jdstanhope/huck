#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v142: the `builtin` builtin.
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
check "builtin echo"        'builtin echo hi'
check "builtin cd"          'builtin cd /tmp; pwd'
check "builtin alone"       'builtin; echo "rc=$?"'
check "cd wrapper"          'cd(){ builtin cd "$@"; }; cd /tmp; pwd'
check "bypass cd fn"        'cd(){ echo SHADOW; }; builtin cd /tmp; pwd'
check "builtin local"       'f(){ builtin local x=5; echo "$x"; }; f'
check "builtin pwd"         'builtin cd /tmp; builtin pwd'
check "command -v builtin"  'command -v builtin'
check "builtin builtin local" 'f(){ builtin builtin local x=5; echo "$x"; }; f'
check "builtin command cd"  'builtin command cd /tmp; pwd'
check "command builtin cd"  'command builtin cd /tmp; pwd'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
