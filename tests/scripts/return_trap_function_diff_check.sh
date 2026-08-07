#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the RETURN trap's function-call
# scoping (#434). Without `functrace` a function does NOT inherit the caller's
# RETURN trap: bash unsets it at function entry (so it is also invisible to
# `trap -p` inside the body) and puts it back on return ONLY if the body left
# RETURN untrapped. A trap the function installs for ITSELF therefore fires on
# its own return with no `set -T`, and outlives the call.
#
# NOT covered here on purpose: a RETURN action that itself runs `return`
# (`trap "echo x; return 3" RETURN`) re-triggers the trap and loops forever in
# real bash — an unbounded-output fragment, not a harness case.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# --- a trap the function sets for itself fires, functrace or not ------------
check "set inside, no -T"    'f() { trap "echo RET" RETURN; echo body; }; f; echo done'
check "set inside, with -T"  'set -T; f() { trap "echo RET" RETURN; echo body; }; f; echo done'
check "set outside, no -T"   'f() { echo body; }; trap "echo RET" RETURN; f; echo done'
check "set outside, with -T" 'set -T; f() { echo body; }; trap "echo RET" RETURN; f; echo done'
check "-T toggled mid-run"   'trap "echo OUT" RETURN; f() { echo body; }; set -T; f; set +T; f; echo end'
check "called twice"         'f() { trap "echo RET" RETURN; echo body; }; f; f; echo done'
check "inside a subshell"    'f() { trap "echo IN" RETURN; echo body; }; ( f ); echo done'

# --- the caller's trap is invisible inside the body -------------------------
check "-p inside body"       'trap "echo OUT" RETURN; f() { trap -p RETURN; }; f; trap -p RETURN'
check "bare trap inside"     'trap "echo OUT" RETURN; f() { trap; }; f; echo done'
check "ignored stays visible" 'trap "" RETURN; f() { trap -p RETURN; echo body; }; f; trap -p RETURN'

# --- restore-on-return only when the body left RETURN untrapped -------------
check "body installs own"    'trap "echo OUT" RETURN; f() { trap "echo IN" RETURN; }; f; trap -p RETURN'
check "body resets"          'trap "echo OUT" RETURN; f() { trap "echo IN" RETURN; trap - RETURN; }; f; trap -p RETURN'
check "body resets only"     'trap "echo OUT" RETURN; f() { trap - RETURN; echo body; }; f; trap -p RETURN'
check "body ignores"         'trap "echo OUT" RETURN; f() { trap "" RETURN; }; f; trap -p RETURN'
check "no caller trap"       'f() { trap "echo IN" RETURN; }; f; trap -p RETURN'

# --- nesting ---------------------------------------------------------------
check "inner sets, outer sees"  'g() { trap "echo G" RETURN; }; f() { g; echo after-g; }; f; echo done'
check "inner sets over caller"  'trap "echo OUT" RETURN; g() { trap "echo G" RETURN; }; f() { g; trap -p RETURN; }; f; trap -p RETURN'
check "both frames set"         'f() { trap "echo F" RETURN; g() { trap "echo G" RETURN; }; g; echo mid; }; f; echo done'
check "recursion"               'f() { trap "echo R$1" RETURN; [ "$1" -gt 0 ] && f $(( $1 - 1 )); return 0; }; f 2; echo done'

# --- status and args seen by the action ------------------------------------
check "explicit return rc"   'f() { trap "echo RET" RETURN; return 7; }; f; echo "rc=$?"'
check "action sees \$?"      'f() { trap "echo st=\$?" RETURN; false; }; f; echo "rc=$?"'
# #441: after an EXPLICIT `return N` the action still sees the status of the
# last command run BEFORE the `return` — N is installed for the caller only.
check "\$? before return N"  'f() { trap "echo st=\$?" RETURN; return 4; }; f; echo "rc=$?"'
check "\$? after a failure"  'f() { trap "echo st=\$?" RETURN; false; return 4; }; f; echo "rc=$?"'
check "\$? bare return"      'f() { trap "echo st=\$?" RETURN; (exit 6); return; }; f; echo "rc=$?"'
check "\$? return under -T"  'set -T; f() { trap "echo st=\$?" RETURN; return 9; }; f; echo "rc=$?"'
check "\$? compound then N"  'f() { trap "echo st=\$?" RETURN; if false; then :; fi; return 5; }; f; echo "rc=$?"'
check "action sees args"     'f() { trap "echo args=\$*" RETURN; echo body; }; f a b; echo done'
check "action sees FUNCNAME" 'f() { trap "echo fn=\${FUNCNAME[0]}" RETURN; echo body; }; f; echo done'
check "action sees local"    'f() { local v=inner; trap "echo v=\$v" RETURN; echo body; }; v=outer; f; echo "after=$v"'
check "action rc ignored"    'f() { trap "false" RETURN; return 3; }; f; echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
