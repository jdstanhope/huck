#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a subscript on something that cannot
# name an array (#609).
#
# A `${…}` subscript is legal on an identifier and nowhere else. bash reports
# everything else as a bad substitution; huck answered EMPTY for the special
# names and rejected a second subscript at PARSE time, which took the whole line
# with it:
#
#     echo ${@[0]}       bash: ${@[0]}: bad substitution   rc 1
#                        huck: (empty)                     rc 0
#     a=(x); echo ${a[0][1]}
#                        bash: ${a[0][1]}: bad substitution        rc 1
#                        huck: syntax error: unsupported expansion rc 2
#
# Three things differed for the second one: the message, the status, and the
# STAGE — bash rejects it while expanding, so the rest of the script still runs.
#
# The length forms (`${#@[0]}`, `${#*[0]}`, `${#1[0]}`) were done in #605 and
# live in `length_modifier_diff_check.sh`; the one row here that pins the two
# rules apart is `${#?[0]}`, which bash answers `0` for while `${?[0]}` is a bad
# substitution (the #608 oddity).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- a subscript on a name that cannot take one ---
check "at name"            'echo ${@[0]}; echo SAME'
check "star name"          'echo ${*[0]}; echo SAME'
check "positional"         'set a b; echo ${1[0]}; echo SAME'
check "last status"        'echo ${?[0]}; echo SAME'
check "dash name"          'echo ${-[0]}; echo SAME'
check "hash name"          'echo ${#[0]}; echo SAME'
check "bang name"          'echo ${![0]}; echo SAME'
check "at with all"        'set a b; echo ${@[@]}; echo SAME'
check "star with index"    'echo ${*[1]}; echo SAME'

# --- a SECOND subscript ---
check "double subscript"   'a=(x y); echo ${a[0][1]}; echo SAME'
check "all then index"     'a=(x y); echo ${a[@][0]}; echo SAME'
check "index then all"     'a=(x y); echo ${a[0][@]}; echo SAME'
check "double on assoc"    'declare -A m=([k]=v); echo ${m[k][0]}; echo SAME'
check "double with length" 'a=(x y); echo ${#a[0][1]}; echo SAME'

# --- the two rules meet: `${#?[0]}` is fine, `${?[0]}` is not ---
check "length on status"   'echo ${#?[0]}; echo SAME'
check "bare on status"     'echo ${?[0]}; echo SAME'

# --- controls: every legal subscript shape ---
check "indexed element"    'a=(x y); echo ${a[0]} ${a[1]} ${a[9]}; echo SAME'
check "indexed all"        'a=(x y); echo ${a[@]} ${a[*]}; echo SAME'
check "indexed length"     'a=(x y); echo ${#a[0]} ${#a[@]}; echo SAME'
check "indexed keys"       'a=(x y); echo ${!a[@]} ${!a[*]}; echo SAME'
check "assoc element"      'declare -A m=([k]=v); echo ${m[k]} ${!m[@]} ${#m[@]}; echo SAME'
check "arith subscript"    'a=(x y); i=1; echo ${a[i]} ${a[$i]} ${a[i-1]} ${a[1+0]}; echo SAME'
check "nested subscript"   'a=(x y); b=(1); echo ${a[${b[0]}]}; echo SAME'
check "scalar subscripted" 'v=abc; echo ${v[0]}; echo SAME'
check "modifier on element" 'a=(x); echo ${a[0]:-D} ${a[1]:-D} ${a[0]#x}; echo SAME'
check "slice of an array"  'a=(x y z); echo ${a[@]:1:2}; echo SAME'
check "unset element"      'a=(x y); unset "a[0]"; echo "[${a[0]}] ${#a[@]}"; echo SAME'

harness_summary
