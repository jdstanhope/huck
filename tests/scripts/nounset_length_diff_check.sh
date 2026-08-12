#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the LENGTH operator under `set -u`
# (#595).
#
#     set -u; echo ${#nope}       bash: nope: unbound variable   huck: 0
#     set -u; echo ${#nope[@]}    bash: nope: unbound variable   huck: 0
#
# huck answered `0` — the length of nothing — where bash treats the missing
# VARIABLE as unbound before the length is asked for. Two details are easy to
# get backwards, so both are rows here:
#
#   * an EMPTY variable is not an unset one: `v=; echo ${#v}` and
#     `a=(); echo ${#a[@]}` are `0` in both shells;
#   * the STATUS differs by shape — a bare `${#nope}` exits 127 under `-c`
#     like any other unbound reference, but the array forms (`${#nope[@]}`,
#     `${#nope[*]}`, `${#nope[0]}`) exit 1.
#
# `$@`/`$*` are exempt (`${#@}` is the positional count, `0` for an empty
# list), as are the special parameters that always have a value; a POSITIONAL
# is not (`${#1}` reports like a name).
#
# The rule is checked in every dispatch context, not just the word one: an
# assignment RHS, a `case` subject, a `[[ ]]` operand and an arithmetic body
# each expand `${…}` through their own path, and wiring a rule into only one
# of them is exactly the #315 shape of bug.
#
# NOT here, each its own divergence and unchanged by this round: a bare
# `declare -a y; echo ${#y[@]}`, which bash calls unbound and huck does not
# because huck materialises a value for a bare `declare` (#600); `${#nope:-D}`,
# which bash rejects as a bad substitution (#605); and a REDIRECTION word
# (`cat <<<${#nope}`), where both shells report the same thing but bash carries
# on and huck exits (#606).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "set -u; $frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "set -u; $frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the scalar form: unbound, status 127 under -c ---
check "scalar length"       'echo ${#nope}; echo SAME'
check "scalar quoted"       'echo "${#nope}"; echo SAME'
check "unset after set"     'v=1; unset v; echo ${#v}; echo SAME'
check "positional"          'echo ${#1}; echo SAME'
check "positional past end" 'set a; echo ${#2}; echo SAME'

# --- the array forms: unbound, status 1 ---
check "all subscript"       'echo ${#nope[@]}; echo SAME'
check "star subscript"      'echo ${#nope[*]}; echo SAME'
check "index subscript"     'echo ${#nope[0]}; echo SAME'
check "index nonzero"       'echo ${#nope[1]}; echo SAME'
check "quoted all"          'echo "${#nope[@]}"; echo SAME'

# --- every dispatch context, not just the word one ---
check "assignment rhs"      'x=${#nope}; echo "x=$x"'
check "local assignment"    'f(){ local x=${#nope}; echo "x=$x"; }; f; echo SAME'
check "case subject"        'case ${#nope} in *) echo c;; esac'
check "double bracket"      '[[ ${#nope} == 0 ]]; echo "rc=$?"'
check "arith body"          'echo $((${#nope})); echo SAME'
check "array literal"       'x=(${#nope}); echo "n=${#x[@]}"'
check "command argument"    'printf "[%s]\n" ${#nope}; echo SAME'
check "assignment array"    'x=${#nope[@]}; echo "x=$x"'

# --- exempt: the specials, and an EMPTY (not unset) variable ---
check "positional count"    'echo ${#@}; echo ${#*}; echo SAME'
check "count with args"     'set a b c; echo ${#@} ${#*}; echo SAME'
check "dollar zero"         'echo ${#0}; echo SAME'
check "last status"         'echo ${#?}; echo SAME'
check "bang no job"         'echo ${#!}; echo SAME'
check "dashes"              'echo ${#-} | wc -c; echo SAME'
check "empty scalar"        'v=; echo ${#v}; echo SAME'
check "empty indexed"       'a=(); echo ${#a[@]}; echo SAME'
check "set scalar"          'v=abc; echo ${#v}; echo SAME'
check "set array"           'a=(x y); echo ${#a[@]} ${#a[0]}; echo SAME'
check "assoc with keys"     'declare -A m=([k]=v); echo ${#m[@]} ${#m[k]}; echo SAME'
check "element unset in set array" 'a=(x y); echo ${#a[5]}; echo SAME'

# --- without set -u, none of this fires ---
check "no nounset scalar"   'set +u; echo ${#nope}; echo SAME'
check "no nounset array"    'set +u; echo ${#nope[@]}; echo SAME'

harness_summary
