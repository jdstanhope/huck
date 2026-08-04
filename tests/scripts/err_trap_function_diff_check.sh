#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the ERR trap around function calls
# (#438, #444). Two rules, both from bash's execute_cmd.c:
#
#   * without `errtrace` (`set -E`) a function does NOT inherit the caller's
#     ERR trap — it is unset for the body (and so invisible to `trap -p`) and
#     restored on return only if the body left ERR untrapped;
#   * `was_error_trap` is captured BEFORE a command runs, so a command that
#     INSTALLS the ERR trap is not itself caught by it.
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

# --- non-inheritance: the caller's trap is unset for the body --------------
check "not inherited"        'trap "echo OUT" ERR; f() { trap -p ERR; false; }; f; echo rc=$?'
check "inherited under -E"   'set -E; trap "echo OUT" ERR; f() { trap -p ERR; false; }; f; echo rc=$?'
check "nested, no -E"        'trap "echo OUT" ERR; g() { false; }; f() { g; }; f; echo rc=$?'
check "nested, with -E"      'set -E; trap "echo OUT" ERR; g() { false; }; f() { g; }; f; echo rc=$?'
check "subshell in body"     'trap "echo OUT" ERR; f() { ( false ); }; f; echo rc=$?'
check "ignored stays"        'trap "" ERR; f() { trap -p ERR; false; }; f; echo rc=$?'
check "bare trap in body"    'trap "echo OUT" ERR; f() { trap; echo listed; }; f; echo done'

# --- restore on return, only when the body left ERR untrapped --------------
check "body installs own"    'trap "echo OUT" ERR; f() { trap "echo IN" ERR; false; }; f; trap -p ERR'
check "body resets"          'trap "echo OUT" ERR; f() { trap "echo IN" ERR; trap - ERR; false; }; f; trap -p ERR'
check "body resets only"     'trap "echo OUT" ERR; f() { trap - ERR; false; }; f; trap -p ERR'
check "body ignores"         'trap "echo OUT" ERR; f() { trap "" ERR; false; }; f; trap -p ERR'
check "no caller trap"       'f() { trap "echo IN" ERR; }; f; trap -p ERR'

# --- armed-before-the-command ----------------------------------------------
check "installer not caught" 'f() { trap "echo IN:\$?" ERR; (exit 2); }; f; (exit 3); echo end'
check "own trap then fail"   'f() { trap "echo IN" ERR; false; }; f; false; echo end'
check "trap then fail, same list" 'trap "echo E" ERR && false; echo rc=$?'
check "installer succeeds"   'f() { trap "echo IN:\$?" ERR; }; f; (exit 3); echo end'

# --- errexit is NOT gated on the trap being armed --------------------------
check "errexit no trap"      'set -e; f() { false; }; f; echo unreached'
check "errexit with trap"    'set -e; trap "echo E" ERR; false; echo unreached'
check "errexit own trap"     'set -e; f() { trap "echo IN" ERR; false; echo unreached-in; }; f; echo unreached'

# --- unchanged neighbours ---------------------------------------------------
check "not fired on \|\|"    'trap "echo E" ERR; false || echo or-ran; echo rc=$?'
check "not fired on !"       'trap "echo E" ERR; ! false; echo rc=$?'
check "subshell fires once"  'trap "echo E" ERR; ( false ); echo rc=$?'
check "while cond no fire"   'trap "echo E" ERR; while false; do :; done; echo rc=$?'
check "if cond no fire"      'trap "echo E" ERR; if false; then :; fi; echo rc=$?'
# NOT covered: a failing command inside `{ }` / `for` / `case` fires ERR twice
# in huck and once in bash. Pre-existing, its own issue (#445).

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
