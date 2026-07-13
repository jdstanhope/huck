#!/usr/bin/env bash
# v285 (#120): the no-pipe foreground-wait fast path must not change any
# observable behavior — output across many external commands and subshells
# (inherited, redirected, and captured stdio) stays byte-identical to bash.
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
# Subshells with inherited stdio (no capture pipe — the fixed path).
check "subshell loop inherited"   'for i in 1 2 3 4 5; do ( echo "s$i" ); done'
check "empty subshell loop"       'for i in 1 2 3; do ( : ); done; echo done'
check "subshell exit status"      '( exit 3 ); echo "rc=$?"'
check "nested subshell"           '( ( ( echo deep ) ) ); echo "rc=$?"'
# External commands with inherited stdio.
check "external loop inherited"   'for i in 1 2 3 4 5; do /bin/echo "e$i"; done'
check "external exit status"      '/bin/false; echo "rc=$?"'
check "external then builtin"     '/bin/true && echo ok'
# Redirected (still no capture pipe on the shell side).
check "subshell redirected"       '( echo hidden ) >/dev/null; echo shown'
check "external redirected"       '/bin/echo hidden >/dev/null; echo shown'
# Captured path must be unchanged too.
check "command substitution"      'x=$( ( echo cap ) ); echo "[$x]"'
check "external in capture"       'x=$(/bin/echo capext); echo "[$x]"'
# Mixed sequence.
check "mixed sequence"            '( echo a ); /bin/echo b; echo c; x=$(echo d); echo "$x"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
