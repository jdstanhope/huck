#!/usr/bin/env bash
# Byte-identical bash<->huck harness for assigning a LIST to a single array
# member (#76): `a[i]=(x y)`.
#
# A compound `(…)` RHS is only valid on the WHOLE array. bash rejects the
# element form with
#
#     a[i]: cannot assign list to array member
#
# naming the lvalue AS WRITTEN — subscript included and UNEXPANDED, so `a[i]`,
# `a[2+3]` and `a[$((1+1))]` each name themselves — and the same wording for an
# associative element as for an indexed one. huck named only the variable and
# used two invented messages (`cannot assign array literal to array element` /
# `… to associative array element`).
#
# Both shells treat this as FATAL under `-c` (the rest of the string does not
# run) and non-fatal from a script file, which is why the rows below carry no
# `echo rc=$?` — the marker is whether anything follows the diagnostic.
#
# The DECLARATION-BUILTIN forms behave the same way (#585): all five report this
# same message and abandon the list. `readonly` and `export` reach it only
# because the LIST rule is checked BEFORE their own rejection of a subscripted
# lvalue — `readonly a[0]=x` is `` `a[0]': not a valid identifier `` while
# `readonly a[0]=(x y)` is this error.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-listmem.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

# DRIVER: `-c` with an explicit $0 so the prologue matches byte for byte.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Script-file driver: the same fragment must NOT abort the file.
check_script() {
    local label="$1" frag="$2" f b h
    f="$TMPROOT/case.sh"
    printf '%s\necho after\n' "$frag" >"$f"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile case.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" case.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- the lvalue is named as written ---
check "literal subscript"   'a[0]=(x y); echo NOTREACHED'
check "name subscript"      'a[i]=(x y); echo NOTREACHED'
check "arith subscript"     'a[2+3]=(x); echo NOTREACHED'
check "expansion subscript" 'a[$((1+1))]=(x); echo NOTREACHED'
check "variable subscript"  'i=3; a[i]=(x); echo NOTREACHED'
check "quoted subscript"    'a["k x"]=(1); echo NOTREACHED'

# --- the array's own state does not change the message ---
check "existing array"      'a=(1 2); a[1]=(x y); echo NOTREACHED'
check "declared indexed"    'declare -a a; a[0]=(x y); echo NOTREACHED'
check "associative"         'declare -A m; m[k]=(x y); echo NOTREACHED'
check "associative expand"  'declare -A m; m[$HOME]=(x); echo NOTREACHED'

# --- the RHS shape does not either ---
check "append form"         'a[0]+=(x y); echo NOTREACHED'
check "empty list"          'a[0]=(); echo NOTREACHED'
check "single element"      'a[0]=(x); echo NOTREACHED'

# --- from a script file it reports and CARRIES ON ---
check_script "script keeps going"   'a[0]=(x y)'
check_script "script assoc"         'declare -A m; m[k]=(x y)'

# --- the declaration builtins: same message, same abandonment (#585) ---
check "declare"             'declare a[0]=(x y); echo NOTREACHED'
check "typeset"             'typeset a[0]=(x y); echo NOTREACHED'
check "declare -a"          'declare -a a[0]=(x y); echo NOTREACHED'
check "readonly"            'readonly a[0]=(x y); echo NOTREACHED'
check "export"              'export a[0]=(x y); echo NOTREACHED'
check "local in a function" 'f(){ local a[0]=(x y); echo NOTREACHED; }; f; echo ALSONOT'
check "readonly name sub"   'readonly a[i]=(x); echo NOTREACHED'
check "export expr sub"     'export a[2+3]=(x); echo NOTREACHED'
check_script "declare in a script" 'declare a[0]=(x y)'
# The list rule is checked BEFORE the subscripted-lvalue rejection those two
# builtins have of their own, so a SCALAR RHS still gets the identifier error.
check "readonly scalar rhs" 'readonly a[0]=x; echo "rc=$?"'
check "export scalar rhs"   'export a[0]=x; echo "rc=$?"'
check "readonly on element" 'a=(p q); readonly a[1]=x; echo "rc=$?"; declare -p a'
# A declaration builtin with a NON-list failure still reports and carries on.
check "declare readonly var" 'readonly r=1; declare r=2; echo AFTER'
check "declare scalar sub"  'declare a[0]=x; echo "rc=$?"; declare -p a'

# --- controls: the valid forms are untouched ---
check "whole array"         'a=(x y); echo "${a[@]}"'
check "whole array append"  'a=(x); a+=(y z); echo "${a[@]}"'
check "element scalar"      'a[0]=x; echo "${a[0]}"'
check "assoc element"       'declare -A m; m[k]=v; echo "${m[k]}"'

harness_summary
