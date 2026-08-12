#!/usr/bin/env bash
# Byte-identical bash<->huck harness for PER-ELEMENT transforms over `$@`, `$*`
# and `${arr[@]}` — the two gaps v340's review left behind (#315, #316).
#
# #315: in a NO-SPLIT context (an assignment RHS, a `case` subject, a `[[ ]]`
# operand) the positional form applied the op to `$1` ONLY:
#
#     set aXa bXb cXc; x=${@/X/-}     bash: a-a b-b c-c    huck: a-a bXb cXc
#
# The splitting context (`echo ${@/X/-}`) was fixed by v340; this is its second
# dispatch site. Note only the SUBSTITUTION op was affected — `^^`, `#`, `%`
# already went through the shared path — which is why every op is a row here.
#
# #316: with an EMPTY IFS an unquoted transform concatenated its results into
# one field, where each element must stay a separate word:
#
#     IFS=; set aXa bXb cXc; recho ${@/X/-}
#     bash: <a-a><b-b><c-c>            huck: <a-ab-bc-c>
#
# The untransformed `${arr[@]}` was already right, so the transform arm was the
# only one joining early. Both arms now hand a word LIST to the render arm and
# let it decide; only a QUOTED `*` joins, which is the row that keeps IFS[0].
#
# `recho` prints each argument in angle brackets, so a lost word boundary is
# visible rather than inferred.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
RECHO='recho(){ for a in "$@"; do printf "<%s>" "$a"; done; echo; }; '

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$RECHO$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$RECHO$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- #315: the no-split contexts ---
check "assign positional sub"  'set aXa bXb cXc; x=${@/X/-}; echo "$x"'
check "assign star sub"        'set aXa bXb cXc; x=${*/X/-}; echo "$x"'
check "assign quoted at"       'set aXa bXb cXc; x="${@/X/-}"; echo "$x"'
check "assign quoted star"     'set aXa bXb cXc; x="${*/X/-}"; echo "$x"'
check "assign case fold"       'set aXa bXb cXc; x=${@^^}; echo "$x"'
check "assign lower"           'set AXA BXB; x=${@,,}; echo "$x"'
check "assign prefix strip"    'set aXa bXb cXc; x=${@#a}; echo "$x"'
check "assign suffix strip"    'set aXa bXb cXc; x=${@%c}; echo "$x"'
check "assign transform U"     'set aXa bXb cXc; x=${@@U}; echo "$x"'
check "assign transform Q"     'set "a b" c; x=${@@Q}; echo "$x"'
check "assign all replace"     'set aXa bXbXb; x=${@//X/-}; echo "$x"'
check "assign array rhs"       'arr=(aXa bXb cXc); x=${arr[@]/X/-}; echo "$x"'
check "assign one param"       'set aXa; x=${@/X/-}; echo "$x"'
check "case subject"           'set aXa bXb cXc; case ${@/X/-} in *b-b*) echo yes;; *) echo no;; esac'
check "double bracket operand" 'set aXa bXb cXc; [[ ${@/X/-} == *"b-b"* ]] && echo yes'
check "assign custom IFS at"   'IFS=,; set aXa bXb cXc; x=${@/X/-}; echo "$x"'
check "assign custom IFS star" 'IFS=,; set aXa bXb cXc; x=${*/X/-}; echo "$x"'
check "assign empty IFS at"    'IFS=; set aXa bXb cXc; x=${@/X/-}; echo "$x"'

# --- #316: word boundaries under an empty IFS ---
check "empty IFS positional"   'IFS=; set aXa bXb cXc; recho ${@/X/-}'
check "empty IFS star"         'IFS=; set aXa bXb cXc; recho ${*/X/-}'
check "empty IFS array"        'IFS=; arr=(aXa bXb cXc); recho ${arr[@]/X/-}'
check "empty IFS array star"   'IFS=; arr=(aXa bXb cXc); recho ${arr[*]/X/-}'
check "empty IFS case fold"    'IFS=; set aXa bXb cXc; recho ${@^^}'
check "empty IFS array fold"   'IFS=; arr=(aXa bXb cXc); recho ${arr[@]^^}'
check "empty IFS untransformed" 'IFS=; arr=(aXa bXb cXc); recho ${arr[@]}'

# --- the shapes that must NOT change ---
check "quoted at keeps words"  'arr=(aXa bXb cXc); recho "${arr[@]/X/-}"'
check "quoted star joins"      'arr=(aXa bXb cXc); recho "${arr[*]/X/-}"'
check "quoted star IFS join"   'IFS=,; arr=(aXa bXb cXc); recho "${arr[*]/X/-}"'
check "quoted positional at"   'set aXa bXb cXc; recho "${@/X/-}"'
check "quoted positional star" 'set aXa bXb cXc; recho "${*/X/-}"'
check "unquoted splits on IFS" 'arr=("a b" cXc); recho ${arr[@]/X/-}'
check "custom IFS unquoted"    'IFS=,; arr=(aXa bXb cXc); recho ${arr[@]/X/-}'
check "splitting context at"   'set aXa bXb cXc; recho ${@/X/-}'
check "empty array"            'arr=(); recho ${arr[@]/X/-}; echo "rc=$?"'
check "substring untouched"    'set aXa bXb cXc; x=${@:1:2}; echo "$x"'

harness_summary
