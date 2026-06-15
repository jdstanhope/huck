#!/usr/bin/env bash
# Byte-identical bash<->huck harness for integer `+=` (arithmetic add, not
# string concatenation) on integer-flagged scalars and array elements.
# (`declare -ai` integer ARRAYS are a separate unimplemented feature and are
# intentionally not exercised here.)
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "int scalar +="        'declare -i s=5; s+=3; echo "$s"'
chk "int += precedence"    'declare -i s=5; s+=2*3; echo "$s"'
chk "int += negative"      'declare -i s=10; s+=-4; echo "$s"'
chk "declare -i after set" 's=5; declare -i s; s+=3; echo "$s"'
chk "int += from unset"    'declare -i s; s+=3; echo "$s"'
chk "int += self-ref"      'declare -i s=5; s+=s; echo "$s"'
chk "int += expr var"      'declare -i s=4; n=3; s+=n*2; echo "$s"'
chk "non-int += concat"    's=5; s+=3; echo "$s"'
chk "int array elem +="    'declare -i a; a[2]=5; a[2]+=4; echo "${a[2]}"'
chk "non-int elem += cat"  'a=(1 2); a[0]+=9; echo "${a[0]}"'
chk "nameref to int +="    'declare -i x=10; declare -n r=x; r+=5; echo "$x"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
