#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v137: a producer writing to a closed
# pipe dies silently (SIGPIPE, status 141) instead of spamming "Broken pipe".
# stdout AND stderr are captured separately and both must match bash.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2"
    local bo be ho he
    bo=$(bash -c "$frag" 2>/tmp/v137_be); be=$(cat /tmp/v137_be)
    ho=$("$HUCK_BIN" -c "$frag" 2>/tmp/v137_he); he=$(cat /tmp/v137_he)
    if [[ "$bo" == "$ho" && "$be" == "$he" ]]; then
        printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else
        printf 'FAIL: %s\n' "$label"
        [[ "$bo" != "$ho" ]] && { echo "  stdout diff:"; diff <(echo "$bo") <(echo "$ho") | sed 's/^/    /'; }
        [[ "$be" != "$he" ]] && { echo "  stderr diff:"; diff <(echo "$be") <(echo "$he") | sed 's/^/    /'; }
        FAIL=$((FAIL+1))
    fi
}
check "printf producer | head"   '{ for i in $(seq 1 5000); do printf "%d\n" "$i"; done; } | head -3'
check "echo producer | head"     '{ for i in $(seq 1 5000); do echo "$i"; done; } | head -3'
check "function producer | head" 'f(){ local i=0; while [ "$i" -lt 5000 ]; do echo "$i"; i=$((i+1)); done; }; f | head -2'
check "subshell producer | head" '( for i in $(seq 1 5000); do echo "$i"; done ) | head -2'
check "external producer | read" 'seq 1 5000 | { read x; echo "first=$x"; }'
check "trap ignore PIPE"         'trap "" PIPE; echo set-ok'
check "trap handler PIPE"        'trap "echo h" PIPE; echo set-ok'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
rm -f /tmp/v137_be /tmp/v137_he
exit $(( FAIL > 0 ? 1 : 0 ))
