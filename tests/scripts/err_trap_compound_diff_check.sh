#!/usr/bin/env bash
# Byte-identical bash<->huck harness for how often the ERR trap fires around
# COMPOUND commands (#445). bash fires at the innermost failing command;
# a compound whose body runs in this process must not fire again for its own
# aggregate status, or the count compounds with nesting — `{ { false; }; }`
# was three fires where bash gives one.
#
# The two kinds that DO fire for themselves are pinned as regression guards:
# a subshell (its forked body cleared its trap table, so nothing fired inside)
# and a function call (the entry-unset from #438 means the body does not fire).
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

# --- a compound does not fire for its own aggregate status ------------------
check "brace group"        'trap "echo E" ERR; { false; }'
check "brace nested"       'trap "echo E" ERR; { { false; }; }'
check "brace 3 deep"       'trap "echo E" ERR; { { { false; }; }; }'
check "brace, last fails"  'trap "echo E" ERR; { true; false; }'
check "brace, first fails" 'trap "echo E" ERR; { false; true; }'
check "brace status kept"  'trap "echo E:\$?" ERR; { (exit 5); }; echo "rc=$?"'
check "for loop"           'trap "echo E" ERR; for i in 1 2; do false; done'
check "for, one iteration" 'trap "echo E" ERR; for i in 1; do false; done'
check "case"               'trap "echo E" ERR; case x in x) false;; esac'
check "case no match"      'trap "echo E" ERR; case x in y) false;; esac; echo "rc=$?"'
check "if then-body fails" 'trap "echo E" ERR; if true; then false; fi'
check "if else-body fails" 'trap "echo E" ERR; if false; then :; else false; fi'
check "if cond only"       'trap "echo E" ERR; if false; then :; fi'
check "while body fails"   'trap "echo E" ERR; while true; do false; break; done'
check "while cond only"    'trap "echo E" ERR; while false; do :; done'
check "until"              'trap "echo E" ERR; until false; do break; done; echo "rc=$?"'
check "arith for"          'trap "echo E" ERR; for ((i=0;i<2;i++)); do false; done'
check "brace in for"       'trap "echo E" ERR; for i in 1; do { false; }; done'
check "brace in function"  'trap "echo E" ERR; f() { { false; }; }; f'

# --- the two that DO fire for themselves (regression guards) ---------------
check "subshell"           'trap "echo E" ERR; ( false )'
check "subshell nested"    'trap "echo E" ERR; ( ( false ) )'
check "function call"      'trap "echo E" ERR; f() { false; }; f'
check "simple command"     'trap "echo E" ERR; false'
check "pipeline"           'trap "echo E" ERR; true | false'
check "arith command"      'trap "echo E" ERR; (( 0 ))'
check "double bracket"     'trap "echo E" ERR; [[ x == y ]]'

# --- errexit is NOT gated on this ------------------------------------------
check "errexit brace"      'set -e; trap "echo E" ERR; { false; }; echo after'
check "errexit for"        'set -e; trap "echo E" ERR; for i in 1; do false; done; echo after'
check "errexit no trap"    'set -e; { false; }; echo after'
check "errexit if body"    'set -e; trap "echo E" ERR; if true; then false; fi; echo after'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
