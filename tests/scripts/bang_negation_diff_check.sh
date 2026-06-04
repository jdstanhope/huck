#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v85 `!` pipeline negation.
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
check "bang false"        '! false; echo $?'
check "bang true"         '! true; echo $?'
check "bang if"           'if ! false; then echo yes; fi'
check "bang while"        'while ! true; do echo x; done; echo done'
check "bang and"          '! false && echo ran'
check "bang pipeline ps"  '! false | true; echo "$? ${PIPESTATUS[@]}"'
check "bang errexit"      'set -e; ! true; echo survived'
check "bang pipefail"     'set -o pipefail; ! false | true; echo $?'
check "bang brace"        '! { false; }; echo $?'
check "bang subshell"     '! (exit 3); echo $?'
check "double bang"       '! ! false; echo $?'
check "test arg bang"     '[ ! -e /nonexistent ]; echo $?'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
