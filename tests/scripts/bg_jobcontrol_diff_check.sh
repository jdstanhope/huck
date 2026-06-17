#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v173 (L-53): non-interactive background
# job control. Stages of a backgrounded job share the shell's process group, but
# kill %n / wait / $! must still work. We assert OBSERVABLE behavior (exit codes
# + stdout) since process-group ids are not byte-stable.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "bgpid nonempty"     'sleep 0.2 & [ -n "$!" ] && echo "have-pid"; wait; echo done'
check "kill %1 running"    'sleep 1 & kill %1; echo "rc=$?"; wait 2>/dev/null; echo done'
check "wait %1 reaps"      'sleep 0.2 & wait %1; echo "rc=$?"'
check "wait \$! exit code" 'sh -c "exit 7" & wait $!; echo "rc=$?"'
check "two bg wait all"    'sleep 0.2 & sleep 0.2 & wait; echo "all done rc=$?"'
check "no job notice"      'sleep 0.2 & wait; echo only-this-line'
check "kill bad spec"      'kill %9 2>/dev/null; echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
