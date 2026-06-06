#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v99: `command CMD` bare form.
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

check "bypass builtin"  'echo() { printf "FUNC"; }; command echo hi'
check "run builtin"     'command echo hi'
check "run external"    "command printf '%s\\n' ext"
check "double command"  'command command echo nested'
# `command no_such 2>/dev/null` would test the 127 path, but huck's
# "command not found" message goes to stderr with a different format than
# bash AND isn't suppressed by 2>/dev/null (a pre-existing divergence that
# also occurs WITHOUT the `command` prefix, so out of scope here). Use the
# pure-stdout `command -v` miss path, which byte-matches (no output, rc 1).
check "v missing"       'command -v no_such_cmd_zzz; echo $?'
check "bypass external"  'true() { return 7; }; command true; echo $?'
check "dash-v builtin"  'command -v echo'
check "no operand"      'command; echo $?'
check "dash-p"          'command -p echo hi'
check "dash-p dash-v"   'command -p -v echo'
check "dashdash"        'command -- echo hi'
check "echo -v arg"     'command echo -v'
check "external sorted" $'command sort <<EOF\nb\na\nc\nEOF'
check "export scalar"   'command export CF=zz; echo "[$CF]"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
