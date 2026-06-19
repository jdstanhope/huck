#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v192: `name=\<NL>(array)` — a line
# continuation between `=`/`+=` and the array `(`.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# the byobu shape: \<NL> between `=` and `(`
check "elem index"    $'arr=\\\n(a b c)\nprintf "%s\\n" "${arr[1]}"'
check "all elems"     $'arr=\\\n(a b c)\nprintf "%s\\n" "${arr[@]}"'
check "count"         $'arr=\\\n(a b c)\necho "${#arr[@]}"'
# append form
check "append"        $'arr=(a); arr+=\\\n(b c)\necho "${arr[2]}"'
# stacked continuations
check "stacked"       $'arr=\\\n\\\n(x y)\necho "${arr[0]}"'
# negative: scalar with continuation (already worked) stays scalar
check "scalar cont"   $'v=\\\nfoo\necho "[$v]"'
# negative: a literal backslash-escape is NOT a continuation
check "escape"        $'v=\\x\necho "[$v]"'
# control: a normal inline array
check "inline array"  'arr=(p q r); echo "${arr[2]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
