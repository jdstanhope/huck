#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v197 bug D: a `\<NL>` line continuation
# BETWEEN array-literal elements (`m=([a]=1 \<NL> [b]=2)`). huck used to read the
# `\<NL>` as the start of a bare element value, producing a spurious no-subscript
# element that broke associative-array initializers with
#   "associative array initializer requires [key]=value form".
# (A real newline inside (...) already worked; only the backslash-newline did not.)
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" script="$2" b h
    b=$(bash -c "$script" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$script" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# $'...' embeds the literal backslash-newline continuations.
check "assoc cont"      $'declare -A m=([a]=1 \\\n [b]=2)\necho ${m[a]}${m[b]}'
check "assoc quoted k"  $'declare -A m=(["/"]=1 \\\n ["/lib"]=1)\necho "${m["/"]}${m["/lib"]}"'
check "blk shape"       $'declare -A m=(["/"]=1 \\\n ["/lib"]=1 \\\n ["/bin"]=1)\necho "${m["/lib"]}${m["/bin"]}"'
check "indexed cont"    $'a=(1 \\\n 2 3)\necho "${a[@]}"'
check "cont after ("    $'declare -A m=( \\\n [a]=1 \\\n )\necho ${m[a]}'
check "real newline"    $'declare -A m=([a]=1\n[b]=2)\necho ${m[a]}${m[b]}'
check "escape not eaten" $'a=(x\\ty)\nprintf "%s\\n" "${a[@]}"'
check "single line"     'declare -A m=([a]=1 [b]=2); echo ${m[a]}${m[b]}'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
