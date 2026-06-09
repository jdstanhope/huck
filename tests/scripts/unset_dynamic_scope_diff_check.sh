#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v118: `unset -v` dynamic-scope
# reveal/pop (M-115). Cases A-H + readonly / unset -f / unset arr[i] guards.
# File-arg execution (L-27: huck history-expands piped stdin).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "A promote across local"  'inner(){ unset -v "$1"; eval $1=VAL; }; mid(){ local x=mv; inner x; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer'
check "B bare unset reveals"    'inner(){ unset -v "$1"; }; mid(){ local x=mv; inner x; echo "m:${x-U}"; }; outer(){ local x=orig; mid x; echo "o:${x-U}"; }; outer'
check "C current-fn-local"      'inner(){ local x=il; unset -v "$1"; eval $1=VAL; }; outer(){ local x=orig; inner x; echo "$x"; }; outer'
check "D three locals"          'leaf(){ unset -v "$1"; eval $1=VAL; }; a(){ local x=av; leaf x; }; b(){ local x=bv; a x; }; outer(){ local x=orig; b x; echo "$x"; }; outer'
check "E global only"           'inner(){ unset -v "$1"; eval $1=VAL; }; x=g; inner x; echo "$x"'
check "F read unset after"      'inner(){ local x=iv; unset -v x; echo "i:${x-U}"; }; outer(){ local x=orig; inner; echo "o:$x"; }; outer'
check "G skip nonlocal frame"   'leaf(){ unset -v "$1"; eval $1=VAL; }; pass(){ leaf "$1"; }; mid(){ local x=mv; pass x; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer'
check "H caller reassigns"      'inner(){ unset -v "$1"; }; mid(){ local x=mv; inner x; x=re; echo "m:$x"; }; outer(){ local x=orig; mid x; echo "o:$x"; }; outer'
check "I reveal unset enclosing" 'inner(){ unset -v "$1"; echo "i:${x-U}"; }; mid(){ local x=mv; inner x; echo "m:${x-U}"; }; outer(){ mid x; echo "o:${x-U}"; }; outer'
check "readonly unset rc"       'readonly r=x; unset r 2>/dev/null; echo "rc=$? r=$r"'
check "unset -f function"       'f(){ echo hi; }; unset -f f; type f >/dev/null 2>&1; echo "rc=$?"'
check "unset array element"     'a=(p q r); unset "a[1]"; echo "${a[*]} n=${#a[@]}"'
check "global unset noreveal"   'g=top; f(){ unset -v g; echo "in:${g-U}"; }; f; echo "out:${g-U}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
