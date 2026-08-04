#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `$?` around a pseudo-signal trap
# action (#437): the action is transparent to `$?`, so the command that
# TRIGGERED the trap is still what the next command sees. bash saves and
# restores last_command_exit_value around the action; huck's shared
# ERR/RETURN fire path did not.
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

# --- ERR: $? after the action is the FAILING command's status --------------
check "succeeding action"   'trap "echo E" ERR; false; echo rc=$?'
check "failing action"      'trap "false" ERR; false; echo rc=$?'
check "action rc differs"   'trap "(exit 3)" ERR; false; echo rc=$?'
check "non-1 status kept"   'trap "echo E" ERR; (exit 5); echo rc=$?'
check "external cmd"        'trap "echo E" ERR; sh -c "exit 4"; echo rc=$?'
check "in a function"       'trap "true" ERR; f() { false; }; f; echo rc=$?'
check "action sees \$?"     'trap "echo st=\$?" ERR; (exit 6); echo rc=$?'
check "\$? survives twice"  'trap "echo E" ERR; false; echo rc1=$?; echo rc2=$?'
check "then a success"      'trap "echo E" ERR; false; true; echo rc=$?'
check "not fired on \|\|"   'trap "echo E" ERR; false || echo or-ran; echo rc=$?'
check "errexit still exits" 'set -e; trap "echo E" ERR; false; echo unreached'
check "\$? into a test"     'trap "echo E" ERR; false; if [ $? -eq 1 ]; then echo one; else echo other; fi'

# --- RETURN: the action must not disturb the call's status -----------------
check "return N kept"       'f() { trap "echo RET" RETURN; return 7; }; f; echo rc=$?'
check "failing RET action"  'f() { trap "false" RETURN; return 7; }; f; echo rc=$?'
check "implicit status"     'f() { trap "true" RETURN; false; }; f; echo rc=$?'

# --- DEBUG already saved/restored; keep it covered here too ----------------
check "DEBUG transparent"   'trap "echo D" DEBUG; false; echo rc=$?'
check "DEBUG failing action" 'trap "false" DEBUG; true; echo rc=$?'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
