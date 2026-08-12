#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v103: set -x xtrace (M-??).
# xtrace goes to stderr; check compares combined stdout+stderr+EXIT.
# Only TOP-LEVEL, depth-1, default-PS4 fragments where huck and bash agree.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
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

# --- #372: a DECLARATION BUILTIN with a compound array value traces as TWO
#     lines — the value on its own, RE-QUOTED (every element single-quoted, an
#     explicit subscript quoted too), then the builtin with the operand reduced
#     to the bare NAME. Note the contrast with the bare assignment rows above,
#     which bash traces as their literal source: `a=(1 2 3)` but
#     `declare -a a=('1' '2' '3')`.
check "declare -a two lines"   'set -x; declare -a a=(x y)'
check "declare -A two lines"   'set -x; declare -A m=([k]=v [j]=w)'
check "declare -a quoted elt"  $'set -x; declare -a a=(\'x y\' z)'
check "declare -a squote elt"  $'set -x; declare -a a=("a\'b")'
check "declare -a empty"       'set -x; declare -a a=()'
check "declare -A empty"       'set -x; declare -A m=()'
check "declare -a subscripts"  'set -x; declare -a a=([3]=x [1]=y)'
check "declare -a append elt"  'set -x; declare -a a=([0]+=x)'
check "declare -A empty value" 'set -x; declare -A m=([k]="")'
check "declare -A spaced key"  'set -x; declare -A m=([a b]="c d")'
check "declare -a plus scalar" 'set -x; declare -a a=(1 2) x=5'
check "declare -a plus bare"   'set -x; declare -a a=(1 2) b'
check "declare -ax flags"      'set -x; declare -ax e=(1)'
check "readonly -a two lines"  'set -x; readonly -a r=(1 2)'
check "export -a two lines"    'set -x; export -a e=(1 2)'
check "typeset -a two lines"   'set -x; typeset -a t=(1 2)'
check "local -a in a function" 'set -x; f(){ local -a a=(1 2); }; f'
check "declare -a then cmd"    'set -x; declare -a a=(1); echo done'
check "declare -a expanded"    'v=1; set -x; declare -a a=($v 2)'
check "declare -A expanded key" 'k=K; set -x; declare -A m=([$k]=v)'
# Untouched: a scalar declare stays on ONE line.
check "declare scalar one line" 'set -x; declare x=1'
check "declare -i one line"    'set -x; declare -i n=5'
#
# NOT covered — the elements are traced BEFORE the assignment's own expansion,
# so an element that word-SPLITS (`declare -a a=($v)` with `v="1 2"`), GLOBS
# (`a=(*.md)`) or holds a command substitution is rendered pre-split and the
# substitution RUNS TWICE. Different root, filed as #581.

harness_summary
