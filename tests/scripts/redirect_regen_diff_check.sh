#!/usr/bin/env bash
# v286 (#121): `declare -f` redirect regeneration must be byte-identical to bash
# for every redirect form (fd>2, <& dup-in, <>, {var}, N>&-, move, ordering).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" body="$2" b h
    b=$(printf 'f() { %s; }\ndeclare -f f\n' "$body" | bash 2>&1)
    h=$(printf 'f() { %s; }\ndeclare -f f\n' "$body" | "$HUCK_BIN" 2>&1)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "trunc default fd"   'true 1>x'
check "trunc fd2"          'true 2>x'
check "trunc fd0"          'true 0>x'
check "trunc fd9"          'true 9>x'
check "read default fd"    'true 0<x'
check "read fd3"           'true 3<x'
check "append fd2"         'true 2>>x'
check "clobber"            'true >|x'
check "readwrite default"  'true <>x'
check "readwrite fd3"      'true 3<>x'
check "dup out default"    'true >&2'
check "dup out fd2"        'true 2>&1'
check "dup in fd3"         'true 3<&0'
check "dup in default"     'true <&0'
check "dup word src in"    'true <&word'
check "dup var src in fd0" 'exec 0<&$fd'
check "dup var src out"    'exec >&$fd'
check "dup word src out"   'true >&2x'
check "readwrite fd1"      'true 1<>x'
check "readwrite fd2"      'true 2<>x'
check "close fd3 out"      'exec 3>&-'
check "close fd3 in"       'exec 3<&-'
check "close default in"   'exec 0<&-'
check "move in fd0"        'exec 0<&5-'
check "move out default"   'true >&2-'
check "move out fd3"       'true 3>&4-'
check "var fd trunc"       'exec {fd}>x'
check "var fd dup"         'exec {v}<&3'
check "var fd move"        'exec {v}<&3-'
check "ordered multi"      'true 3>&1 4<&0'
check "mixed order"        'true >a 2>&1'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
