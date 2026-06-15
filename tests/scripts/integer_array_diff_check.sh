#!/usr/bin/env bash
# Byte-identical bash<->huck harness for integer ARRAYS (`declare -ai` /
# `declare -Ai` / `local -ai` / `local -Ai`): every element VALUE is
# arith-evaluated on assignment (keys of associative arrays are NOT coerced).
# Non-integer arrays must stay literal. (L-49)
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

chk() { local l="$1" f="$2" b h
    b=$(bash --norc --noprofile -c "$f" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

chk "ai literal coerces"    'declare -ai a=(2+3 4*5); echo "${a[@]}"'
chk "ai element assign"     'declare -ai a; a[0]=3+4; echo "${a[0]}"'
chk "ai append coerces"     'declare -ai a=(1); a+=(2*3 8); echo "${a[@]}"'
chk "ai element +="         'declare -ai a=(5); a[0]+=3; echo "${a[0]}"'
chk "ai declare -p"         'declare -ai a=(2+3); declare -p a'
# Bad arith on an unset var name coerces to 0 (bash + huck agree). A trailing
# operator like `7+` is a hard syntax error in bash (declare aborts, rc 1) and
# is intentionally NOT tested here — huck would coerce it to 0 (L-49 note).
chk "ai bad arith -> 0"     'declare -ai a=(xyz); echo "${a[0]}"'
chk "Ai value coerces"      'declare -Ai m=([x]=2+3 [y]=10); echo "${m[x]} ${m[y]}"'
chk "Ai element assign"     'declare -Ai m; m[k]=6/2; echo "${m[k]}"'
chk "non-int array literal" 'a=(2+3); echo "${a[0]}"'
chk "local -ai in func"     'f(){ local -ai a=(1+1 2+2); echo "${a[@]}"; }; f'
chk "local -Ai in func"     'f(){ local -Ai m=([a]=3*3); echo "${m[a]}"; }; f'
chk "ai negative + ref"     'declare -ai a=(10); a[0]+=-3; echo "${a[0]}"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
