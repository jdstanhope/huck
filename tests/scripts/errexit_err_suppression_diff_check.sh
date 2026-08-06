#!/usr/bin/env bash
# Byte-identical bash<->huck harness for how an EXEMPT command's body treats
# the ERR trap and `set -e` (#480, #468, #469, #470).
#
# bash propagates "ignore return" INTO the body of a command whose own failure
# is exempt. huck applied the exemption only at the outer command, so a body
# still tripped `set -e` — the shell exited where bash ran the handler.
#
# The one asymmetric cell: `!` does NOT stop a compound body firing ERR unless
# errexit is on. Confirmed as the inner command firing, not an artefact:
# `! { (exit 5); }` reports E:5, and `! { false; true; }` fires though the
# group SUCCEEDS.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- errexit inside an exempt body: the sharp end (#480) -------------------
check "errexit, func via ||"   'set -e; f() { false; echo x; }; f || echo or'
check "errexit, func via &&"   'set -e; f() { false; echo x; }; true && f; echo after'
check "errexit, brace via ||"  'set -e; { false; echo x; } || echo or'
check "errexit, brace via &&"  'set -e; { false; echo x; } && echo and; echo after'
check "errexit, for via ||"    'set -e; for i in 1; do false; echo x; done || echo or'
check "errexit, nested via ||" 'set -e; { { false; echo x; }; } || echo or'
check "errexit still exits"    'set -e; f() { false; echo x; }; f; echo after'
check "errexit plain"          'set -e; false; echo after'

# --- ERR in an exempt body (#468) ------------------------------------------
check "ERR, brace via ||"      'trap "echo E" ERR; { false; } || echo or'
check "ERR, brace via &&"      'trap "echo E" ERR; { false; } && echo and'
check "ERR, body prints"       'trap "echo E" ERR; { false; echo x; } || echo or'
check "ERR, for via ||"        'trap "echo E" ERR; for i in 1; do false; done || echo or'
check "ERR, status still 1"    'trap "echo E" ERR; { false; } || echo "rc=$?"'
check "ERR, if cond"           'trap "echo E" ERR; if { false; }; then :; fi; echo after'
check "ERR, while cond"        'trap "echo E" ERR; while { false; }; do :; done; echo after'

# --- the `!` asymmetry (#469) ----------------------------------------------
check "! fires without -e"     'trap "echo E" ERR; ! { false; }; echo after'
check "! silent with -e"       'set -e; trap "echo E" ERR; ! { false; }; echo after'
check "! carries the status"   'trap "echo E:\$?" ERR; ! { (exit 5); }'
check "! group succeeds"       'trap "echo E" ERR; ! { false; true; }'
check "! nested"               'trap "echo E" ERR; ! { { false; }; }'
check "! double negation"      'trap "echo E" ERR; ! ! { false; }'
check "! simple command"       'trap "echo E" ERR; ! false'
check "! subshell"             'trap "echo E" ERR; ! ( false )'

# --- the inherited path under set -E (#470) --------------------------------
check "-E func via ||"         'set -E; trap "echo E" ERR; f() { false; }; f || echo or'
check "-E func plain"          'set -E; trap "echo E" ERR; f() { false; }; f; echo after'
check "-E func negated"        'set -E; trap "echo E" ERR; f() { false; }; ! f'
check "-E brace via ||"        'set -E; trap "echo E" ERR; { false; } || echo or'

# --- rules that must NOT change --------------------------------------------
check "last command fires"     'trap "echo E" ERR; { false; }'
check "compound once (#445)"   'trap "echo E" ERR; { { false; }; }'
check "subshell fires"         'trap "echo E" ERR; ( false )'
check "function call fires"    'trap "echo E" ERR; f() { false; }; f'
check "errexit in if body"     'set -e; if true; then false; fi; echo after'
check "status w/o a trap"      '{ false; } || echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
