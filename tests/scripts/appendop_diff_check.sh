#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the `+=` APPEND-OPERATOR roots fixed in
# v344 (#327): associative `arr+=scalar` appending to key [0], integer array
# element `+=` doing arithmetic addition (not concat), and runtime
# POSIXLY_CORRECT toggling posix mode (special-builtin prefix-assignment
# persistence). Each fragment runs through `bash -c` and `huck -c`; stdout +
# stderr + exit must match byte-for-byte.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- Root A: associative arr+=scalar appends to key [0] ---
check "assoc scalar append"      'declare -A f=([a]=1); f+=zero; echo "${f[0]}"'
check "assoc append keeps others" 'declare -A f=([a]=1); f+=zero; echo "${f[a]} ${f[0]}"'
check "assoc int scalar append"  'declare -Ai f; f+=5; echo "${f[0]}"'
check "assoc non-append to [0]"  'declare -A f=([a]=1); f=zero; echo "a=${f[a]} 0=${f[0]}"'

# --- Root B: integer indexed/assoc element += is arithmetic, not concat ---
check "int indexed arr+=n"       'declare -ai a=(2 2 3); a+=1; echo "${a[0]}"'
check "int indexed a[i]+=n"      'declare -ai a=(2 2 3); a[0]+=1; echo "${a[0]}"'
check "int indexed grows"        'declare -ai a=(10); a+=5; a[0]+=100; echo "${a[0]}"'
check "int assoc a[k]+=n"        'declare -Ai m=([k]=10); m[k]+=5; echo "${m[k]}"'
check "non-int indexed arr+=z"   'a=(x y); a+=z; echo "${a[0]}"'
check "non-int indexed a[i]+=z"  'a=(x y); a[1]+=z; echo "${a[1]}"'
# empty RHS on integer array += is base+0 (keeps base), not a reset to 0
check "int indexed arr+= empty"  'declare -ai a=(10 20); a+=""; echo "${a[0]}"'
check "int indexed a[i]+= empty" 'declare -ai a=(10 20); a[0]+=""; echo "${a[0]}"'
check "int assoc a[k]+= empty"   'declare -Ai m=([k]=10); m[k]+=""; echo "${m[k]}"'
check "int indexed += unset"     'declare -ai a=(10); u=; a+=$u; echo "${a[0]}"'
check "int indexed += rhs expr"  'declare -ai a=(10); a+=2+3; echo "${a[0]}"'

# --- Root C: runtime POSIXLY_CORRECT toggles posix mode ---
check "runtime posixly persist"  'POSIXLY_CORRECT=1; x=2; x+=5 eval "echo hi"; echo "$x"'
check "no posix no persist"      'x=2; x+=5 eval "echo hi"; echo "$x"'
check "unset posixly restores"   'POSIXLY_CORRECT=1; unset POSIXLY_CORRECT; x=2; x+=5 eval "echo hi"; echo "$x"'

# --- scalar += regression guards (must be unchanged) ---
check "scalar concat +="         's=ab; s+=cd; echo "$s"'
check "int scalar +="            'declare -i n=5; n+=3; echo "$n"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
