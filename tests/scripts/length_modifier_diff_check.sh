#!/usr/bin/env bash
# Byte-identical bash<->huck harness for a modifier after `${#name}` (#605).
#
# `${#name}` takes a subscript and then the closing brace, and nothing else.
# bash rejects everything else as a bad substitution; huck parsed the `#` as the
# length operator and then applied the modifier anyway, answering something
# plausible:
#
#     v=abc; echo ${#v:-D}    bash: ${#v:-D}: bad substitution   huck: abc
#     v=abc; echo ${#v#a}     bash: ${#v#a}: bad substitution    huck: bc
#     v=abc; echo ${#v:1:1}   bash: ${#v:1:1}: bad substitution  huck: b
#
# The message names the whole WORD, not the `${…}` alone (`echo x${#v:-D}y`
# reports `x${#v:-D}y`), which huck already did for other bad substitutions.
#
# One shape is FATAL where the rest are not: `${#@…}` stops the script, while
# `${#v…}` and even `${#*…}` report and carry on. Both statuses are here, from
# a script file as well as `-c`, since that is where the difference shows.
#
# The rows that must NOT change are the length forms that are legal — a
# subscript is fine (`${#a[0]}`, `${#a[@]}`), as are the specials whose name IS
# the `#` (`${#}`, `${##}`, `${#-}`, `${#?}`, `${#!}`).
#
# A SUBSCRIPT follows the same rule: legal on a name that could name an array
# (`${#a[0]}`), a bad substitution on `@`, `*` or a positional.
#
# NOT here: `${#?:-D}` and `${#-:-D}`, which bash answers `0` for (it stops
# reading the name at the special and treats the rest as an operator on `$#`)
# where huck calls both a bad substitution — #608; and a DOUBLE subscript
# (`${a[0][1]}`), which bash calls a bad substitution and huck rejects at parse
# time with its own message — #609.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"
TMPROOT=$(mktemp -d "${TMPDIR:-/tmp}/huck-lenmod.XXXXXX")
trap 'rm -rf "$TMPROOT"' EXIT

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# A script file shows the FATALITY that `-c` folds into a status code.
check_file() {
    local label="$1" frag="$2" b h
    printf '%s\n' "$frag" >"$TMPROOT/f.sh"
    b=$(cd "$TMPROOT" && timeout 10 "$BASH_BIN" --norc --noprofile f.sh 2>&1; echo "EXIT:$?")
    h=$(cd "$TMPROOT" && timeout 10 "$HUCK_BIN" f.sh 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- every modifier in the table, after a length prefix ---
check "use default colon"  'v=abc; echo ${#v:-D}; echo SAME'
check "use default bare"   'v=abc; echo ${#v-D}; echo SAME'
check "assign default"     'v=abc; echo ${#v:=D}; echo SAME'
check "error if unset"     'v=abc; echo ${#v:?m}; echo SAME'
check "use alternate"      'v=abc; echo ${#v:+D}; echo SAME'
check "remove prefix"      'v=abc; echo ${#v#a}; echo SAME'
check "remove prefix long" 'v=abc; echo ${#v##a}; echo SAME'
check "remove suffix"      'v=abc; echo ${#v%c}; echo SAME'
check "substitute"         'v=abc; echo ${#v/a/z}; echo SAME'
check "substitute all"     'v=abc; echo ${#v//a/z}; echo SAME'
check "upper"              'v=abc; echo ${#v^^}; echo SAME'
check "lower"              'v=ABC; echo ${#v,,}; echo SAME'
check "transform Q"        'v=abc; echo ${#v@Q}; echo SAME'
check "transform A"        'v=abc; echo ${#v@A}; echo SAME'
check "substring"          'v=abc; echo ${#v:1:1}; echo SAME'
check "substring no len"   'v=abc; echo ${#v:1}; echo SAME'
check "on an unset name"   'echo ${#nope:-D}; echo SAME'
check "after a subscript"  'a=(x y); echo ${#a[0]:-D}; echo SAME'
check "after all subscript" 'a=(x y); echo ${#a[@]:-D}; echo SAME'
check "positional name"    'set a b; echo ${#1:-D}; echo SAME'
check "star name"          'echo ${#*:-D}; echo SAME'
check "star subscript"     'echo ${#*[0]}; echo SAME'
check "positional subscript" 'set a b; echo ${#1[0]}; echo SAME'

# --- the message names the whole word ---
check "embedded in a word" 'v=abc; echo x${#v:-D}y; echo SAME'
check "inside quotes"      'v=abc; echo "x${#v:-D}y"; echo SAME'
check "two in one word"    'v=abc; echo ${#v:-D}${#v#a}; echo SAME'
check "in an assignment"   'v=abc; x=${#v:-D}; echo "rc=$? x=$x"'

# --- `${#@…}` is the fatal one, in both drivers ---
check "at name colon"      'echo ${#@:-D}; echo SAME'
check "at name pattern"    'echo ${#@#a}; echo SAME'
check "at name subscript"  'echo ${#@[0]}; echo SAME'
check "at name with args"  'set a b; echo ${#@:-D}; echo SAME'
check_file "at name in a script" 'echo ${#@:-D}
echo SAME'
check_file "name in a script"    'v=abc; echo ${#v:-D}
echo SAME'
check_file "star in a script"    'echo ${#*:-D}
echo SAME'

# --- controls: the legal length forms ---
check "plain length"       'v=abc; echo ${#v}; echo SAME'
check "length quoted"      'v=abc; echo "${#v}"; echo SAME'
check "element length"     'a=(x yy); echo ${#a[1]}; echo SAME'
check "array count"        'a=(x yy); echo ${#a[@]} ${#a[*]}; echo SAME'
check "assoc value length" 'declare -A m=([k]=vv); echo ${#m[k]} ${#m[@]}; echo SAME'
check "arg count"          'set a b; echo ${#} ${##}; echo SAME'
check "special names"      'echo ${#-} ${#?} ${#!} ${#0}; echo SAME'
check "positional length"  'set abc; echo ${#1}; echo SAME'
check "count of positional" 'set a b; echo ${#@} ${#*}; echo SAME'
check "modifier no length" 'v=abc; echo ${v:-D} ${v#a} ${v:1:1}; echo SAME'
check "indirect unchanged" 'v=abc; w=v; echo ${!w}; echo SAME'
check "nested expansion"   'v=abc; a=(x); echo ${#a[${#v}]}; echo SAME'

harness_summary
