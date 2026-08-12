#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a BAD ARRAY SUBSCRIPT (#572).
#
# One mistake, and bash answers it five different ways depending on where it
# appears — a different label each time, and two different fatalities:
#
#   ${a[-3]}      value       `a: bad array subscript`        reports, CARRIES ON,
#                                                             element treated as unset
#   ${#a[-3]}     length      `-3]: bad array subscript`      fatal (note the `]`)
#   a[-3]=z       assignment  `a[-3]: bad array subscript`    fatal
#   a=([-3]=z)    literal     `[-3]=z: bad array subscript`   reports, SKIPS the
#                                                             element, rc 0
#   unset 'a[-3]' unset       `unset: [-3]: bad array …`      reports, status 1
#
# huck named the VARIABLE in all five and made the value form fatal, so
# `${a[-3]:-D}` neither reported nor substituted the default, and an array
# literal lost its remaining elements.
#
# A subscript whose ARITHMETIC fails is a different error entirely: every form
# reports the ordinary arith diagnostic (`1+: syntax error: operand expected …`)
# and every form is fatal — including `unset`, which drops its own prefix there.
#
# `set -u` beats the subscript question: an unset VARIABLE is unbound before the
# subscript is looked at, and the element form names the subscript as written
# (`nonexistent[-1]: unbound variable`).
#
# Both shells run with an EXPLICIT $0 ("huck5") so the prologue matches byte for
# byte. Each fragment ends with a marker so "did the list keep running" is
# visible in the output, not just in the status.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the VALUE form: reported, non-fatal, element treated as unset ---
check "value plain"        'a=(x y); echo ${a[-3]}; echo SAME'
check "value quoted"       'a=(x y); echo "${a[-3]}"; echo SAME'
check "value unset var"    'echo ${nonexistent[-1]}; echo SAME'
check "value default"      'a=(x y); echo ${a[-3]:-D}; echo SAME'
check "value default unset" 'echo ${nonexistent[-1]:-D}; echo SAME'
check "value alternate"    'a=(x y); echo ${a[-3]+A}; echo SAME'
check "value trim"         'a=(x y); echo ${a[-3]#x}; echo SAME'
check "value in assignment" 'a=(x y); x=${a[-3]}; echo "rc=$? x=[$x]"; echo SAME'
check "value deep negative" 'a=(x y); echo ${a[-99]}; echo SAME'

# --- the LENGTH form: `<subscript>]`, fatal ---
check "length array"       'a=(x y); echo ${#a[-3]}; echo SAME'
check "length scalar"      'v=abc; echo ${#v[-1]}; echo SAME'
check "length expr sub"    'a=(x y); echo ${#a[1-9]}; echo SAME'
check "length var sub"     'a=(x y); i=-3; echo ${#a[i]}; echo SAME'

# --- assignment, array literal, unset: three more labels ---
check "assignment"         'a=(x y); a[-3]=z; echo "rc=$? SAME"'
check "assignment append"  'a=(x y); a[-3]+=z; echo "rc=$? SAME"'
check "literal element"    'a=(x y); a=([-3]=z); echo "rc=$? SAME"'
check "literal append ctx" 'a=(x y); a+=([-3]=z); echo "rc=$? SAME"'
check "literal keeps rest" 'a=(); a=([-3]=z [1]=ok); echo "rc=$? n=${#a[@]} one=${a[1]}"'
check "unset element"      'a=(x y); unset "a[-3]"; echo "rc=$? SAME"'

# --- an ARITHMETIC failure in the subscript: arith diagnostic, always fatal ---
check "arith value"        'a=(x y); echo ${a[1+]}; echo SAME'
check "arith length"       'a=(x y); echo ${#a[1+]}; echo SAME'
check "arith div by zero"  'a=(x y); echo ${a[1/0]}; echo SAME'
check "arith assignment"   'a=(x y); a[1+]=z; echo "rc=$? SAME"'
check "arith literal"      'a=([1+]=z); echo "rc=$? SAME"'
check "arith unset"        'a=(x y); unset "a[1+]"; echo "rc=$? SAME"'

# --- set -u: the variable is unbound before the subscript matters ---
check "nounset length"     'set -u; echo ${#nonexistent[-1]}; echo SAME'
check "nounset length 0"   'set -u; echo ${#nonexistent[0]}; echo SAME'
check "nounset value"      'set -u; echo ${nonexistent[-1]}; echo SAME'
check "nounset set array"  'set -u; a=(x y); echo ${#a[-3]}; echo SAME'
check "nounset missing el" 'set -u; a=(x y); echo ${a[5]}; echo SAME'
check "nounset empty array" 'set -u; a=(); echo ${#a[0]}; echo SAME'

# --- controls: the forms that were always fine ---
check "good subscript"     'a=(x y); echo ${a[1]}; echo ${#a[1]}; echo SAME'
check "negative in range"  'a=(x y); echo ${a[-1]}; echo ${#a[-1]}; echo SAME'
check "past the end"       'a=(x y); echo "[${a[9]}]"; echo ${#a[9]}; echo SAME'
check "assoc negative key" 'declare -A m=([k]=v); echo "[${m[-3]}]"; echo ${#m[-3]}; echo SAME'
check "unset good element" 'a=(x y); unset "a[1]"; echo "rc=$? n=${#a[@]}"'

harness_summary
