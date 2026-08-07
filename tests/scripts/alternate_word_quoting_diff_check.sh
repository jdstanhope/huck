#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v114: alternate/default word quoting
# under an unquoted outer ${param+word}/${param-word} (M-110). Quoted-outer is
# unchanged (regression guard).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check "array empty elem"       'a=(x "" y); set -- ${a[@]+"${a[@]}"}; echo $#; printf "<%s>" "$@"; echo'
check "array spaced elem"      'a=("a b" c); set -- ${a[@]+"${a[@]}"}; echo $#'
check "scalar quoted inner"    'x=1; set -- ${x+"a b"}; echo $#'
check "scalar unquoted inner"  'x=1; set -- ${x+a b}; echo $#'
check "fully-quoted array"     'a=(x "" y); set -- "${a[@]+"${a[@]}"}"; echo $#'
check "fully-quoted scalar"    'x=1; set -- "${x+a b}"; echo $#'
check "default unset quoted"   'unset u; set -- ${u-"a b"}; echo $#'
check "default unset unquoted" 'unset u; set -- ${u-a b}; echo $#'
check "assoc spaced value"     'declare -A m=([k]="a b"); set -- ${m[@]+"${m[@]}"}; echo $#'
check "upvars mise shape"      'words=(mise ""); set -- -a${#words[@]} words ${words+"${words[@]}"} -v cword 1; echo $#'

harness_summary
