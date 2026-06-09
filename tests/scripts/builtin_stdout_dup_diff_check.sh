#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v124 Fix B: builtins honor a `>&N`
# stdout redirect. File-arg execution (L-27). Compares stdout only (2>/dev/null
# both sides) so the huck:/bash: error-prefix divergence is irrelevant.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>/dev/null; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "echo>&2 captured empty"  'a=$(echo Z >&2); echo "[$a]"'
check "printf>&2 captured empty" 'a=$(printf "%s\n" Z >&2); echo "[$a]"'
check "echo>&1 stays stdout"     'echo KEEP >&1'
check "func >&2 under 2>/dev/null" 'f() { >&2 printf "%s\n" MSG; }; a=$( (f 2>/dev/null) ); echo "[$a]"'
check "echo>&- discards"         'a=$(echo GONE >&-); echo "[$a]"'
check "two builtins one >&2"     'a=$( { echo A; echo B >&2; } ); echo "[$a]"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
