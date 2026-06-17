#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v175: bash-legal function-name characters
# (-, ., :, +, leading digit) across all three definition forms. Each positive
# case DEFINES and CALLS the function, asserting identical stdout+exit. Negative
# regressions assert BOTH shells reject the fragment when RUN (exit codes may
# differ — bash 1 vs huck 2 — and the error WORDING legitimately differs:
# `huck:` vs `bash: line N:`; only both-nonzero matters).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

check() {  # label ; fragment — assert byte-identical stdout+stderr+exit
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

both_reject() {  # label ; fragment — assert BOTH shells reject it (nonzero exit when RUN)
    local label="$1" frag="$2" bn hn
    printf '%s\n' "$frag" | bash --norc --noprofile >/dev/null 2>&1; bn=$?
    printf '%s\n' "$frag" | "$HUCK_BIN" >/dev/null 2>&1; hn=$?
    if [[ "$bn" != 0 && "$hn" != 0 ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s (bash rc=%s huck rc=%s; both should be nonzero)\n' "$label" "$bn" "$hn"; FAIL=$((FAIL+1)); fi
}

check "dash name"        'foo-bar() { echo "[$1]"; }; foo-bar X'
check "dot name"         'foo.bar() { echo dot; }; foo.bar'
check "colon name"       'ns:fn() { echo colon; }; ns:fn'
check "plus name"        'a+b() { echo plus; }; a+b'
check "leading digit"    '2foo() { echo digit; }; 2foo'
check "function kw dash" 'function fzf-widget { echo kw; }; fzf-widget'
check "function kw paren" 'function f-g() { echo kwparen; }; f-g'
check "plain identifier"  'foo_bar() { echo id; }; foo_bar'
both_reject "reserved name rejected" 'if() { :; }'
both_reject "hyphen for-var rejected" 'for a-b in 1; do :; done'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
