#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v103: set -x xtrace (M-??).
# xtrace goes to stderr; check compares combined stdout+stderr+EXIT.
# Only TOP-LEVEL, depth-1, default-PS4 fragments where huck and bash agree.
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

check "trace echo"     'set -x; echo hi'
check "trace expanded" 'x=hi; set -x; echo "$x" a'
check "enable disable" 'set -x; echo a; set +x; echo b'
# NOTE: "dash has x" dropped — verifying $- contains x requires tracing an
# assignment (`d=$-`) or a [[/case compound, neither of which huck traces, and
# $- content differs anyway. Integration tests already cover $- having x.
check "set -o xtrace"  'set -o xtrace; echo hi'
check "trace true"     'set -x; true; set +x; echo done'
check "trace two args" 'set -x; echo one two'

# v339 (#310) Root 1: arith-for section trace preserves trailing whitespace.
check "arith-for trailing sp"  'set -x; for ((i=0; i<=2; i++ )); do :; done'
check "arith-for no sp"        'set -x; for ((i=0;i<=2;i++)); do :; done'
check "arith-for all spaced"   'set -x; for ((i=0 ; i<=2 ; i++ )); do :; done'
# declare -f reconstruction shares the same section-trim path.
check "declare -f arith-for"   'f() { for ((i=0; i<=2; i++ )); do :; done; }; declare -f f'

# v339 (#310) Root 3: BASH_XTRACEFD redirects xtrace to an fd; unset reverts.
check "BASH_XTRACEFD" 'tf=$(mktemp); exec 4>"$tf"; BASH_XTRACEFD=4; set -x; echo a; echo b; unset BASH_XTRACEFD; echo c; set +x; echo ---; cat "$tf"; rm -f "$tf"'

# v339 (#310) Root 2: standalone assignment trace shows the operator (+=/=) and
# the RHS this statement assigned, not the full post-append value.
check "trace plain assign"     'set -x; x=hi'
check "trace append assign"    'set -x; foo=one; foo+=two'
check "trace append expand"    'y=world; set -x; foo=hello; foo+=" $y"'

# #311: an array-literal assignment traces as its LITERAL source (unexpanded,
# original quoting), not the first element / expanded values.
check "trace array assign"     'set -x; a=(1 2 3)'
check "trace array append"     'a=(1 2); set -x; a+=(3 4)'
check "trace array source"     'x=hi; set -x; a=($x)'
check "trace array quoted elt" $'set -x; a=(\'a b\' c)'
check "trace array subscripts" 'set -x; a=([2]=x [5]=y)'
check "trace assoc assign"     'declare -A m; set -x; m=([k]=v [j]=w)'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
