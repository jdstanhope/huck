#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `${#name[subscript]}` (#491).
#
# The rule the issue turns on: on a variable that is entirely UNSET, the length
# of an element is `0` and NOTHING is raised — bash never evaluates the
# negative-subscript wrap, which is the part that has no answer without a
# maximum index. huck evaluated it, failed, reported `bad array subscript` and
# abandoned the rest of the command list (and before v358, exited the shell).
#
# A variable that IS set is a different case and unchanged: `a=(x y); ${#a[-3]}`
# really is a bad subscript in bash too, and abandons the list on both sides.
# Those rows are here as controls, comparing the STATUS and the abandonment
# only — the diagnostic itself still differs in which name it prints, which is
# #572 along with the value form's fatality and the `set -u` precedence.
#
# Every fragment ends with a marker so "did the list keep running" is visible in
# the output rather than only in the exit status.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

# DRIVER: `-c` with an explicit $0 ("huck5") so any prologue matches.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# stderr dropped: the diagnostic's NAME still differs (#572), the control flow
# does not.
check_quiet() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>/dev/null; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>/dev/null; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- #491: an unset variable answers 0 and keeps going ---
check "unset [-1]"          'echo ${#nonexistent[-1]}; echo SAME'
check "unset [-5]"          'echo ${#nonexistent[-5]}; echo SAME'
check "unset [0]"           'echo ${#nonexistent[0]}; echo SAME'
check "unset [1]"           'echo ${#nonexistent[1]}; echo SAME'
check "unset after unset"   'v=1; unset v; echo ${#v[-1]}; echo SAME'
check "unset quoted"        'echo "${#nonexistent[-1]}"; echo SAME'
check "unset in assignment" 'x=${#nonexistent[-1]}; echo "[$x]"; echo SAME'
check "unset in a function" 'f(){ echo ${#u[-1]}; }; f; echo SAME'
check "unset arith subscript" 'echo ${#nonexistent[1-4]}; echo SAME'
check "unset status"        'echo ${#nonexistent[-1]}; echo "rc=$?"'

# --- a SET variable is unchanged: still a bad subscript, still fatal ---
check_quiet "array [-3] of 2"  'a=(x y); echo ${#a[-3]}; echo SAME'
check_quiet "scalar [-1]"      'v=abc; echo ${#v[-1]}; echo SAME'

# --- controls: the forms that already worked ---
check "array [-1]"          'a=(x y); echo ${#a[-1]}; echo SAME'
check "array [-2]"          'a=(x y); echo ${#a[-2]}; echo SAME'
check "array past end"      'a=(x y); echo ${#a[5]}; echo SAME'
check "array count"         'a=(x y); echo ${#a[@]}; echo SAME'
check "unset count"         'echo ${#nonexistent[@]}; echo SAME'
check "assoc missing key"   'declare -A m; echo ${#m[nope]}; echo SAME'
check "assoc negative key"  'declare -A m; echo ${#m[-1]}; echo SAME'
check "scalar [0]"          'v=abc; echo ${#v[0]}; echo SAME'
check "plain length"        'v=abc; echo ${#v}; echo SAME'

harness_summary
