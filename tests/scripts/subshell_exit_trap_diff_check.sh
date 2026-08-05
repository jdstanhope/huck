#!/usr/bin/env bash
# Byte-identical bash<->huck harness for an EXIT trap a CHILD installs for
# itself (#449). All four child kinds — `( )`, `&`, a pipeline stage and
# `$( )` — funnel through one fork helper, so this covers one code path from
# four directions.
#
# The inherited case is the other half and must NOT change: a child does not
# fire the PARENT's EXIT trap (huck already matched bash), so those rows are
# regression guards, not new behaviour.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Both sides time-boxed — a child that mishandles its exit path can hang.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- a child-installed EXIT trap fires, in every child kind ----------------
check "plain subshell"    '( trap "echo t" EXIT; echo b )'
check "background"        '( trap "echo t" EXIT; echo b ) & wait'
check "pipeline stage"    '{ trap "echo t" EXIT; echo x; } | cat'
check "pipeline first"    '{ trap "echo t" EXIT; echo x; } | sort'
check "command sub"       'echo "[$( trap "echo t" EXIT; echo b )]"'
check "nested subshell"   '( ( trap "echo t" EXIT; echo b ) )'
check "subshell in func"  'f() { ( trap "echo t" EXIT; echo b ); }; f'
check "two children"      '( trap "echo t1" EXIT; : ); ( trap "echo t2" EXIT; : )'

# --- status propagation ----------------------------------------------------
check "trap exits child"  '( trap "exit 9" EXIT; echo b ); echo "rc=$?"'
check "explicit exit"     '( trap "echo t" EXIT; exit 3 ); echo "rc=$?"'
check "body status kept"  '( trap "echo t" EXIT; false ); echo "rc=$?"'
check "comsub status"     'x=$( trap "echo t; exit 9" EXIT; echo b ); echo "[$x] rc=$?"'
check "comsub captures"   'x=$( trap "echo t" EXIT; echo b ); echo "[$x]"'

# --- the trap's own output goes where the child's output goes --------------
check "stderr from trap"  '( trap "echo t >&2" EXIT; echo b ) 2>/dev/null'
check "redirected child"  '( trap "echo t" EXIT; echo b ) > /dev/null; echo done'

# --- the parent's EXIT trap is NOT inherited (regression guards) -----------
check "not inherited"     'trap "echo T" EXIT; ( echo sub )'
check "not in comsub"     'trap "echo T" EXIT; echo "[$(echo body)]"'
check "parent unchanged"  'trap "echo T" EXIT; ( exit 5 ); echo "rc=$?"'
check "child then parent" 'trap "echo P" EXIT; ( trap "echo C" EXIT; echo b )'
check "parent fires once" 'trap "echo P" EXIT; ( : ); ( : ); echo done'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
