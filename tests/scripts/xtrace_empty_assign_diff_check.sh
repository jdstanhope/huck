#!/usr/bin/env bash
# Byte-identical bash<->huck harness for an EMPTY value in an xtrace assignment
# line (#614).
#
#     set -x; x=       bash: + x=        huck: + x=''
#
# An empty command ARGUMENT really is `''` in bash (`echo ""` traces as
# `+ echo ''`), which is why huck quoted both; the assignment word is the
# exception, and it is the same in every assignment path — a bare `x=`, an
# inline prefix, `declare`, `local`, `+=`, and a value that came back empty
# from an expansion or a command substitution.
#
# Every non-empty value already matched, so the controls below are the ones that
# must not change: a space is still `' '`, a metacharacter still quotes, an
# array literal still traces as its source.
#
# NOT here: `export e=` and `readonly r=`, where bash emits a SECOND trace line
# (`+ export e=` then `+ e=`) that huck does not — a declaration-builtin tracing
# divergence, #581's family, unrelated to the quoting.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "set -x; $frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "set -x; $frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- an empty assignment value, in every path ---
check "bare empty"         'x='
check "empty quotes"       'x=""'
check "empty single"       "x=''"
check "unset variable"     'x=$u'
check "empty comsub"       'x=$(true)'
check "empty printf"       'x=$(printf "")'
check "inline prefix"      'v= true'
check "prefix among many"  'v="" w=x true'
check "declare"            'declare v='
check "declare integer"    'declare -i v='
check "local"              'f(){ local q=; }; f'
check "append empty"       'v=; v+='
check "append to value"    'v=a; v+='
check "two on a line"      'x=; y='
check "empty then used"    'x=; echo "[$x]"'

# --- controls: everything non-empty, and the argument case ---
check "empty argument"     'echo ""'
check "empty among args"   'echo "" a ""'
check "space value"        'x=" "'
check "spaced value"       'x="a b"'
check "meta value"         'x="a*b"'
check "plain value"        'x=a'
check "array literal"      'x=(a "" b)'
check "empty array"        'x=()'
check "comsub value"       'x=$(echo hi)'
check "value then read"    'v=x; echo "$v"'

harness_summary
