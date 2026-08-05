#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `trap - SIG` restoring the signal's
# default disposition (#474). Unregistering the action left signal_hook's own
# handler installed, which silently turned the signal into "caught and does
# nothing" — so a re-untrapped SIGTERM stopped killing the shell.
#
# Each fragment kills the shell with the signal under test and prints `alive`
# only if it survived, so the assertion is the shell's life, not its output.
#
# NOT covered, and excluded from the fix on purpose:
#  - SIGQUIT: bash IGNORES it in a non-interactive shell, so `trap - QUIT`
#    restores IGNORE, not default. huck has no such startup disposition yet
#    (#478), so restoring SIG_DFL there would create a new divergence.
#  - SIGINT / SIGCHLD: huck registers always-on flag handlers for Ctrl-C
#    polling and child reaping; those must survive `trap - INT` / `trap - CHLD`.
#    Both are pinned below as regression guards instead.
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

# --- reset restores the default: the signal kills the shell again ----------
check "USR1 after reset"   'trap "echo x" USR1; trap - USR1; kill -USR1 $$; sleep 0.3; echo alive'
check "USR2 after reset"   'trap "echo x" USR2; trap - USR2; kill -USR2 $$; sleep 0.3; echo alive'
check "TERM after reset"   'trap "echo x" TERM; trap - TERM; kill -TERM $$; sleep 0.3; echo alive'
check "HUP after reset"    'trap "echo x" HUP; trap - HUP; kill -HUP $$; sleep 0.3; echo alive'
check "ALRM after reset"   'trap "echo x" ALRM; trap - ALRM; kill -ALRM $$; sleep 0.3; echo alive'
check "reset after ignore" 'trap "" USR1; trap - USR1; kill -USR1 $$; sleep 0.3; echo alive'
check "reset twice"        'trap "echo x" USR1; trap - USR1; trap - USR1; kill -USR1 $$; sleep 0.3; echo alive'
check "reset by number"    'trap "echo x" 10; trap - 10; kill -10 $$; sleep 0.3; echo alive'

# --- states that must NOT change -------------------------------------------
check "still trapped"      'trap "echo x" USR1; kill -USR1 $$; sleep 0.3; echo alive'
check "ignored stays"      'trap "" USR1; kill -USR1 $$; sleep 0.3; echo alive'
check "never trapped"      'kill -USR1 $$; sleep 0.3; echo alive'
check "retrap after reset" 'trap "echo x" USR1; trap - USR1; trap "echo y" USR1; kill -USR1 $$; sleep 0.3; echo alive'
check "listing after reset" 'trap "echo x" USR1; trap - USR1; trap -p USR1; echo listed'

# --- huck's own machinery survives a reset (regression guards) -------------
check "reaping after CHLD reset" 'trap "echo x" CHLD; trap - CHLD; sleep 0.2 & wait; echo reaped-ok'
check "INT reset still dies"     'trap "echo x" INT; trap - INT; kill -INT $$; sleep 0.3; echo alive'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
