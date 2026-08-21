#!/usr/bin/env bash
# Byte-identical bash<->huck harness for an INVALID arithmetic value written to
# an integer-flagged variable (#712).
#
# bash reports the arithmetic error, leaves the variable ALONE, and raises the
# ordinary arithmetic-expansion failure — so under `-c` the rest of the command
# list is discarded, exactly as for a failing `$(( ))`. huck coerced silently to
# `0`, exited 0, and carried on:
#
#     declare -i v=5; v=@        bash: syntax error …    huck: v becomes 0, rc 0
#
# The silent `0` was the dangerous half: `declare -i total; total=$(compute)`
# could not tell a genuine zero from a parse failure.
#
# The same silent-zero appeared separately in the integer `+=` paths, which
# evaluated each operand with `.unwrap_or(0)`.
#
# COMPARED: the diagnostic with the `$0`/`line N:` prefix stripped, plus stdout
# and status. Both `-c` (where the failure discards the rest of the list) and a
# SCRIPT FILE (where only the failing command is discarded) are driven, since
# those differ.
#
# NOT compared here:
#   - the diagnostic when the coercion happens under `declare`/`export`/
#     `readonly`: bash prefixes it with the builtin name (`declare: @: syntax
#     error …`), huck does not (#714). Those three rows compare status and
#     stdout only. Every plain-assignment form — including the silent-zero one
#     this issue is about — agrees in full.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

# `-c` driver: the whole string is one command list.
check_c() {
    local label="$1" frag="$2" b h
    b=$( bash --norc --noprofile -c "$frag" sh 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'
         bash --norc --noprofile -c "$frag" sh 2>/dev/null; echo "EXIT:$?" )
    h=$( "$HUCK_BIN" -c "$frag" sh 2>&1 >/dev/null | sed 's/^.*line [0-9]*: //'
         "$HUCK_BIN" -c "$frag" sh 2>/dev/null; echo "EXIT:$?" )
    compare "-c  $label" "$b" "$h"
}

# Script-file driver: only the failing command is discarded, so the lines after
# it still run and the surviving variable value is observable.
check_file() {
    local label="$1" b h tmp
    tmp=$(mktemp)
    cat > "$tmp"
    b=$( bash --norc --noprofile "$tmp" 2>&1 >/dev/null | sed 's/^[^ ]*: line [0-9]*: //'
         bash --norc --noprofile "$tmp" 2>/dev/null; echo "EXIT:$?" )
    h=$( "$HUCK_BIN" "$tmp" 2>&1 >/dev/null | sed 's/^[^ ]*: line [0-9]*: //'
         "$HUCK_BIN" "$tmp" 2>/dev/null; echo "EXIT:$?" )
    rm -f "$tmp"
    compare "file $label" "$b" "$h"
}

# Status and stdout only — for the shapes whose MESSAGE PREFIX is #714.
check_rc() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" sh 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" sh 2>/dev/null; echo "EXIT:$?")
    compare "-c  $label" "$b" "$h"
}

# --- a plain assignment to an integer scalar ---
check_c 'scalar: bad operand'   'declare -i v=5; v=@; echo "rc=$?"; declare -p v'
check_c 'scalar: division by 0' 'declare -i v=5; v=1/0; echo "rc=$?"'
check_c 'scalar: trailing +'    'declare -i v=5; v=1+; echo "rc=$?"'
check_rc 'scalar: at declare'   'declare -i v=@; echo "rc=$?"'
check_rc 'scalar: via export'   'declare -i v=5; export v=@; echo "rc=$?"'
check_rc 'scalar: via readonly' 'declare -i v=5; readonly v=@; echo "rc=$?"'
check_c 'scalar: fresh integer' 'declare -i v; v=@; echo "rc=$?"'

# --- the integer `+=` paths, which had their own silent zero ---
check_c 'append: scalar'        'declare -i v=5; v+=@; echo "rc=$?"'
check_c 'append: array element' 'declare -ia a=(1 2); a[0]+=@; echo "rc=$?"'
check_c 'literal: array elem'   'declare -ia a=(1 2); a=(3 @); echo "rc=$?"'
check_c 'element: assignment'   'declare -ia a=(1 2); a[0]=@; echo "rc=$?"'
check_c 'assoc: element'        'declare -Ai m; m[k]=@; echo "rc=$?"'

# --- the variable KEEPS its previous value, and the script carries on ---
check_file 'scalar keeps value' <<'FRAG'
declare -i v=5
v=@
echo AFTER
declare -p v
FRAG
check_file 'append keeps value' <<'FRAG'
declare -i v=5
v+=@
echo AFTER
declare -p v
FRAG
check_file 'element keeps value' <<'FRAG'
declare -ia a=(1 2)
a[0]=@
echo AFTER
declare -p a
FRAG
check_file 'inside a function' <<'FRAG'
declare -i v=5
f(){ v=@; echo IN; }
f
echo AFTER
declare -p v
FRAG

# --- valid arithmetic is unaffected ---
check_c 'ok: expression'        'declare -i v=5; v=2+3; declare -p v'
check_c 'ok: append arith'      'declare -i v=5; v+=3*2; declare -p v'
check_c 'ok: element arith'     'declare -ia a; a[1]=2+3; declare -p a'
check_c 'ok: element append'    'declare -ia a=(5); a[0]+=3; declare -p a'
check_c 'ok: assoc arith'       'declare -Ai m; m[k]=2+3; declare -p m'
check_c 'ok: empty is zero'     'declare -i v=5; v=; declare -p v'
check_c 'ok: append to empty'   'declare -i v; v+=4; declare -p v'
check_c 'ok: non-integer var'   'v=@; echo "rc=$? v=$v"'
check_c 'ok: string append'     'v=a; v+=@; echo "rc=$? v=$v"'

harness_summary
