#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v328 (#273): the RETURN trap is
# suppressed for a function/sourced-script return that happens WHILE the
# DEBUG trap action is itself executing (bash suppresses RETURN for the
# duration of the DEBUG action). ERR does NOT suppress RETURN, and
# RETURN-during-RETURN is already guarded by fire_pseudo_trap's own
# recursion check (pre-existing, unrelated to this gate).
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

check "return supp in debug action"      'set -T; helper(){ return 0; }; trap "helper" DEBUG; trap "echo RET" RETURN; echo cmd'
check "return supp multi-cmd debug"      'set -T; helper(){ return 0; }; trap "helper; :" DEBUG; trap "echo RET" RETURN; echo cmd'
check "return fires in err action"       'set -T; helper(){ return 0; }; trap "helper; echo inerr" ERR; trap "echo RET" RETURN; false'
check "real return still fires (-T)"     'set -T; f(){ echo body; }; trap "echo RET" RETURN; f'
check "real return still fires (extdbg)" 'shopt -s extdebug; f(){ echo body; }; trap "echo RET" RETURN; f'
# NOTE: pdt's body is a silent no-op (`:`), not a visible echo. A visible
# per-firing echo here would make this check depend on the pre-existing,
# OUT-OF-SCOPE #274 gap (bash fires one extra DEBUG "before the first command
# executes in a shell function" under functrace/extdebug that huck does not
# yet implement — see functrace_diff_check.sh's header comment) rather than
# on the RETURN-suppression behavior this harness targets. pdt is still a
# real function invoked by the DEBUG trap with $LINENO (the print_debug_trap
# "shape"), so the trap-dispatch machinery is still exercised end to end.
check "print_debug_trap shape"           'set -T; pdt(){ :; }; prt(){ echo "ret $1"; }; trap "pdt \$LINENO" DEBUG; trap "prt \$LINENO" RETURN; g(){ echo g; }; g'
check "no functrace: no return"          'f(){ echo body; }; trap "echo RET" RETURN; f'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
