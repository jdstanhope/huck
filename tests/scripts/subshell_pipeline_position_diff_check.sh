#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v100: subshell-headed pipeline in any position (M-11a).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# A subshell `( ... )` that heads a pipeline, in every command position.
check "sub pipe ;"        'echo z; ( echo a ) | sort'
check "sub pipe &&"       "true && ( printf 'b\\na\\n' ) | sort"
check "sub pipe ||"       'false || ( echo x ) | cat'
check "brace pipe ;"      'echo z; { echo a; echo b; } | sort'
check "if pipe ;"         'echo z; if true; then echo a; fi | cat'
check "fn body sub pipe"  'f() { echo z; ( echo a ) | sort; }; f'
check "for body sub pipe" 'for i in 1 2; do ( echo $i ) | cat; done'
check "negated sub pipe"  'echo z; ! ( false ) | cat; echo $?'
check "mixed mid compound" 'echo z; ( echo a ) | { cat; } | cat'
check "negated after &&"  'true && ! ( false ) | cat; echo $?'
check "first-pos regress" '( echo a ) | sort; echo z'
check "plain seq regress" 'echo a; echo b; true && echo y'
check "nvm shape"         $'f() {\n  local X\n  ( for n in b a; do echo $n & done; wait ) | sort\n}\nf'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
