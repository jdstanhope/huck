#!/usr/bin/env bash
# Byte-identical bash<->huck harness for substitution with an EMPTY pattern
# (#448). An empty pattern matches nothing anywhere — EXCEPT at a single-slash
# anchor, where the empty match lands at that end and the replacement is
# inserted: `${x/#/pre}` prepends, `${x/%/suf}` appends, and over `${a[@]}`
# it does so per element. The replace-all forms (`${x//#/pre}`, `${x///-}`)
# stay no-ops, as does an empty replacement.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# --- the anchored empty pattern inserts ------------------------------------
check "prefix insert"      's=abc; echo "[${s/#/pre}]"'
check "suffix insert"      's=abc; echo "[${s/%/-suf}]"'
check "prefix on empty"    's=; echo "[${s/#/pre}]"'
check "suffix on empty"    's=; echo "[${s/%/suf}]"'
check "prefix, one char"   's=a; echo "[${s/#/pre}]"'
check "replacement w/ IFS" 's=abc; echo "[${s/#/a b}]"'
check "unquoted prefix"    's=abc; echo [${s/#/pre}]'
check "in a for loop"      'for w in a b; do echo "${w/#/>}"; done'

# --- no-op cases -----------------------------------------------------------
check "empty replacement"  's=abc; echo "[${s/#}]"'
check "empty repl suffix"  's=abc; echo "[${s/%}]"'
check "all + prefix"       's=abc; echo "[${s//#/pre}]"'
check "all + suffix"       's=abc; echo "[${s//%/suf}]"'
check "bare empty pattern" 's=abc; echo "[${s//}]"'
check "single slash empty" 's=abc; echo "[${s/}]"'
check "all, empty w/ repl" 's=abc; echo "[${s///-}]"'

# --- arrays apply it per element -------------------------------------------
check "array @ prefix"     'a=(x y); echo "${a[@]/#/pre-}"'
check "array @ suffix"     'a=(x y); echo "${a[@]/%/-suf}"'
check "array * prefix"     'a=(x y); echo "${a[*]/#/pre-}"'
check "array quoted @"     'a=("x 1" y); printf "%s\n" "${a[@]/#/p-}"'
check "assoc @ prefix"     'declare -A m=([k]=v); echo "${m[@]/#/pre-}"'
check "array empty elem"   'a=(x "" y); printf "[%s]" "${a[@]/#/p}"; echo'

# --- neighbours that must not change ---------------------------------------
check "non-empty prefix"   's=abc; echo "[${s/#a/A}]"'
check "non-empty suffix"   's=abc; echo "[${s/%c/C}]"'
check "prefix no match"    's=abc; echo "[${s/#z/Z}]"'
check "plain substitute"   's=abc; echo "[${s/b/B}]"'
check "replace all"        's=abab; echo "[${s//b/B}]"'
check "array non-empty"    'a=(ax bx); echo "${a[@]/x/Z}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
