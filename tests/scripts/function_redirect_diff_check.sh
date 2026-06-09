#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v125 (M-117): redirections on a
# function-call command apply to the body. File-arg execution (L-27).
# stdout-only compare (2>/dev/null both sides).
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

check "func >file"            'f(){ printf "%s\n" BODY; }; d=$(mktemp -d); f >"$d/x"; cat "$d/x"'
check "func >&2 not captured" 'f(){ printf "%s\n" BODY; }; a=$(f >&2); echo "[$a]"'
check "func >>file append"    'f(){ printf "%s\n" L; }; d=$(mktemp -d); f >"$d/x"; f >>"$d/x"; cat "$d/x"'
check "inline-assign + redir" 'f(){ printf "%s\n" "v=$V"; }; d=$(mktemp -d); V=9 f >"$d/x"; cat "$d/x"'
check "builtin+external body" 'f(){ echo B; command echo X; }; d=$(mktemp -d); f >"$d/x"; cat "$d/x"'
check "func <herestring"      'r(){ read a; echo "got=$a"; }; r <<< "hi"'
# NOTE: "func 2>&1 captures" (b=$(g 2>&1)) is L-25: execute_capturing uses an
# in-process Capture sink (Rust Vec), not a real fd-1 pipe; dup2(1,2) cannot
# redirect into the capture buf. Bash forks for $(), making 2>&1 work there.
# This case is NOT included until L-25 is resolved.

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
