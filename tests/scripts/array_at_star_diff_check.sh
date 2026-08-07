#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v334: unquoted `${arr[@]}`/`${arr[*]}`
# field-splitting under an EMPTY IFS. Under IFS='', bash keeps each array
# element a separate word (no re-joining, no splitting); huck previously
# joined the elements into a single word (empty IFS[0] separator collapsed
# them). Matrix + empty-element + surrounding-text + associative cases, each
# bash-verified against `bash --norc --noprofile` first.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# matrix: {${A[@]}, ${A[*]}, "${A[@]}", "${A[*]}"} x {IFS='', ' ', '/', unset}
#   A=(bob 'tom dick harry' joe); set <expr>; echo "$#|$1|$2|$3"
setup='A=(bob "tom dick harry" joe)'

check "IFS=''  \${A[@]}"     "$setup; IFS=''; set -- \${A[@]}; echo \"\$#|\$1|\$2|\$3\""
check "IFS=''  \${A[*]}"     "$setup; IFS=''; set -- \${A[*]}; echo \"\$#|\$1|\$2|\$3\""
check "IFS=''  \"\${A[@]}\"" "$setup; IFS=''; set -- \"\${A[@]}\"; echo \"\$#|\$1|\$2|\$3\""
check "IFS=''  \"\${A[*]}\"" "$setup; IFS=''; set -- \"\${A[*]}\"; echo \"\$#|\$1|\$2|\$3\""

check "IFS=' ' \${A[@]}"     "$setup; IFS=' '; set -- \${A[@]}; echo \"\$#|\$1|\$2|\$3\""
check "IFS=' ' \${A[*]}"     "$setup; IFS=' '; set -- \${A[*]}; echo \"\$#|\$1|\$2|\$3\""
check "IFS=' ' \"\${A[@]}\"" "$setup; IFS=' '; set -- \"\${A[@]}\"; echo \"\$#|\$1|\$2|\$3\""
check "IFS=' ' \"\${A[*]}\"" "$setup; IFS=' '; set -- \"\${A[*]}\"; echo \"\$#|\$1|\$2|\$3\""

check "IFS='/' \${A[@]}"     "$setup; IFS='/'; set -- \${A[@]}; echo \"\$#|\$1|\$2|\$3\""
check "IFS='/' \${A[*]}"     "$setup; IFS='/'; set -- \${A[*]}; echo \"\$#|\$1|\$2|\$3\""
check "IFS='/' \"\${A[@]}\"" "$setup; IFS='/'; set -- \"\${A[@]}\"; echo \"\$#|\$1|\$2|\$3\""
check "IFS='/' \"\${A[*]}\"" "$setup; IFS='/'; set -- \"\${A[*]}\"; echo \"\$#|\$1|\$2|\$3\""

check "unset  \${A[@]}"      "$setup; unset IFS; set -- \${A[@]}; echo \"\$#|\$1|\$2|\$3\""
check "unset  \${A[*]}"      "$setup; unset IFS; set -- \${A[*]}; echo \"\$#|\$1|\$2|\$3\""
check "unset  \"\${A[@]}\""  "$setup; unset IFS; set -- \"\${A[@]}\"; echo \"\$#|\$1|\$2|\$3\""
check "unset  \"\${A[*]}\""  "$setup; unset IFS; set -- \"\${A[*]}\"; echo \"\$#|\$1|\$2|\$3\""

# empty-element counts: A=(a '' b); set ${A[@]}; echo $#   (IFS '', ' ', '/')
check "empty-elem IFS=''"  'A=(a "" b); IFS=""; set -- ${A[@]}; echo $#'
check "empty-elem IFS=' '" 'A=(a "" b); IFS=" "; set -- ${A[@]}; echo $#'
check "empty-elem IFS='/'" 'A=(a "" b); IFS="/"; set -- ${A[@]}; echo $#'

# surrounding text: IFS=''; A=(p q); set x${A[@]}y; echo "$#|$1|$2"  -> 2|xp|qy
check "surrounding text" 'IFS=""; A=(p q); set -- x${A[@]}y; echo "$#|$1|$2"'

# associative: IFS=''; declare -A m=([x]=bob [y]='t d' [z]=joe); set ${m[*]}; echo $#  -> 3
check "assoc IFS='' \${m[*]}" 'IFS=""; declare -A m=([x]=bob [y]="t d" [z]=joe); set -- ${m[*]}; echo $#'

harness_summary
