#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v334: unquoted `${arr[@]}`/`${arr[*]}`
# field-splitting under an EMPTY IFS. Under IFS='', bash keeps each array
# element a separate word (no re-joining, no splitting); huck previously
# joined the elements into a single word (empty IFS[0] separator collapsed
# them). Matrix + empty-element + surrounding-text + associative cases, each
# bash-verified against `bash --norc --noprofile` first.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
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

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
