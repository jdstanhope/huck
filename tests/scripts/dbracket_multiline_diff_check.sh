#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v87: multi-line [[ ]] + test ops (M-14a).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
: > "$FIX/old"; sleep 1; : > "$FIX/new"; ln "$FIX/new" "$FIX/link"

# Multi-line continuation (real newlines inside the fragment). Only break in
# the positions bash accepts: after `[[`, after `&&`/`||`, and before `]]`.
check "break before ]]"   $'[[ -f /etc/passwd\n]] && echo yes'
check "break after &&"    $'[[ -f /etc/passwd &&\n -f /etc/hosts ]] && echo both'
check "break after [["    $'[[\n -f /etc/passwd ]] && echo opened'
check "single-line still"  '[[ -f /etc/passwd ]] && echo ok'
check "echo [[ literal"    'echo [['
# -v
check "v set"              'x=1; [[ -v x ]] && echo S || echo U'
check "v empty"            'y=; [[ -v y ]] && echo S || echo U'
check "v unset"            'unset z; [[ -v z ]] && echo S || echo U'
check "test -v set"        'x=1; [ -v x ] && echo S || echo U'
check "test -v unset"      'unset z; [ -v z ] && echo S || echo U'
# -nt/-ot/-ef
check "nt"                 "[[ '$FIX/new' -nt '$FIX/old' ]] && echo nt || echo no"
check "ot"                 "[[ '$FIX/old' -ot '$FIX/new' ]] && echo ot || echo no"
check "ef hardlink"        "[[ '$FIX/new' -ef '$FIX/link' ]] && echo ef || echo no"
check "ef different"       "[[ '$FIX/old' -ef '$FIX/new' ]] && echo ef || echo no"
check "nt missing rhs"     "[[ '$FIX/new' -nt '$FIX/missing' ]] && echo nt || echo no"
check "test nt"            "[ '$FIX/new' -nt '$FIX/old' ] && echo nt || echo no"
# Bare-word truthiness: [[ word ]] ≡ [[ -n word ]]  (M-14c, v92)
check "bareword nonempty"  '[[ foo ]]; echo $?'
check "bareword empty"     '[[ "" ]]; echo $?'
check "bareword var set"   's=x; [[ $s ]]; echo $?'
check "bareword var empty" 'e=; [[ $e ]]; echo $?'
check "bareword var unset" 'unset u; [[ $u ]]; echo $?'
check "bareword and"       '[[ a && b ]]; echo $?'
check "bareword or empty"  '[[ "" || z ]]; echo $?'
check "bareword and empty" '[[ a && "" ]]; echo $?'
check "bareword not empty" '[[ ! "" ]]; echo $?'
check "bareword grouped"   '[[ ( a ) ]]; echo $?'
check "bareword op wins"   '[[ word == x ]]; echo $?'
check "bareword op match"  '[[ word == word ]]; echo $?'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
