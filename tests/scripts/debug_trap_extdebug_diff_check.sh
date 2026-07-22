#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v322 (#255): honoring DebugDecision
# (Proceed/SkipCommand/ReturnFromSub) at both simple-command dispatch sites
# (Exec + bare Assign). Covers: DEBUG firing before a bare assignment,
# $LINENO reframed to the pending command's line inside the trap action,
# FUNCNAME inside the action, and extdebug's status-1 skip / status-2
# simulated-return (both at top level and inside a function).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# check_script runs the fragment as a SCRIPT FILE (not -c). Needed for the
# main-script-is-not-a-returnable-subroutine case: at the top level of a script
# run as an argument, an extdebug status-2 DEBUG trap skips one command (it does
# NOT simulate a return / abort the script) — a distinction invisible in -c mode.
check_script() {
    local label="$1" frag="$2" b h f
    f=$(mktemp)
    printf '%s\n' "$frag" > "$f"
    b=$(bash --norc "$f" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "EXIT:$?")
    rm -f "$f"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "fires before assignments" 'n=0; trap '"'"'n=$((n+1))'"'"' DEBUG; x=1; y=2; echo $n'
check "lineno tracks pending cmd" 'trap '"'"'echo L=$LINENO'"'"' DEBUG
echo one
echo two'
check "funcname main from action" 'trap '"'"'echo ${FUNCNAME[1]:-none}-${FUNCNAME[0]:-none}'"'"' DEBUG; :'
check "extdebug top-level skip (rc2)" 'shopt -s extdebug; de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 2; fi; return 0; }; trap '"'"'tr'"'"' DEBUG; x=1; de=2; x=2; echo x=$x'
check "extdebug top-level skip (rc1)" 'shopt -s extdebug; de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 1; fi; return 0; }; trap '"'"'tr'"'"' DEBUG; x=1; de=2; x=2; echo x=$x'
check "extdebug func return (rc2)" 'shopt -s extdebug; de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 2; fi; return 0; }; trap '"'"'tr'"'"' DEBUG; f(){ echo A; de=2; echo B; echo C; }; f; echo "ret=$?"'
check "extdebug func skip-one (rc1)" 'shopt -s extdebug; de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 1; fi; return 0; }; trap '"'"'tr'"'"' DEBUG; f(){ echo A; de=2; echo B; echo C; }; f; echo "ret=$?"'
check "no extdebug: no skip" 'de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 2; fi; return 0; }; trap '"'"'tr'"'"' DEBUG; x=1; de=2; x=2; echo x=$x'
# Action calls a function that reads $LINENO: the function's own $LINENO must
# NOT be shifted by the trap-action line reframe (regression guard).
check "action fn reads own lineno" 'pt(){ echo in=$LINENO; }; trap '"'"'pt'"'"' DEBUG
echo one
echo two'
# Script-file mode: extdebug status-2 at the top level of the MAIN script skips
# one command and CONTINUES (does not simulate a return that aborts the script).
check_script "script-mode top-level skip" 'shopt -s extdebug
de=0; tr(){ if [[ $de == 2 ]]; then de=0; return 2; fi; return 0; }
trap '"'"'tr'"'"' DEBUG
x=1; de=2; x=2; echo "x=$x cont"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
