#!/usr/bin/env bash
# Byte-identical bash<->huck harness for `exit` performed BY a trap action
# (#442). Every fire site used to discard the action's outcome, so
# `trap 'exit 1' ERR` — the standard abort-on-error idiom — silently carried on.
#
# Two rules constrain the mechanism and are pinned below: the EXIT trap still
# fires when another trap's `exit` ends the shell (so the unwind cannot
# short-circuit), and the LAST exit wins (the EXIT trap can override an earlier
# request because it runs last).
#
# NOT covered on purpose: `return` inside a RETURN trap, which re-enters until
# real bash dies with `xmalloc: cannot allocate 16 bytes`. Reproducing memory
# exhaustion is not a compatibility goal (docs/bash-divergences.md).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# Both sides run under `timeout`: several fragments here are only bounded
# BECAUSE the trap exits, so a shell that ignores the exit (the bug being
# fixed) would spin forever — `trap "exit 9" ERR; while true; do false; done`
# is an infinite loop in a shell that swallows the request. A hung harness is
# far worse than a failing one.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 bash -c "$frag" 2>&1; echo "rc=$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# --- all five trap kinds honour `exit N` ------------------------------------
check "EXIT kind"        'trap "exit 9" EXIT; true'
check "ERR kind"         'trap "exit 9" ERR; false; echo after'
check "DEBUG kind"       'trap "exit 9" DEBUG; echo a; echo after'
check "RETURN kind"      'f() { trap "exit 9" RETURN; :; }; f; echo after'
check "RETURN under -T"  'set -T; trap "exit 9" RETURN; f() { :; }; f; echo after'
check "signal kind"      'trap "exit 9" USR1; kill -USR1 $$; sleep 0.2; echo after'

# --- bare `exit` uses $? as of that moment ----------------------------------
check "bare exit EXIT"   'trap "exit" EXIT; (exit 4); true'
check "bare exit ERR"    'trap "exit" ERR; (exit 6); echo after'

# --- the unwind escapes every nesting form ----------------------------------
check "from a function"  'trap "exit 9" ERR; f() { false; }; f; echo after'
check "from a loop"      'trap "exit 9" ERR; i=0; while [ $i -lt 3 ]; do false; i=$((i+1)); done; echo after'
check "from a for loop"  'trap "exit 9" ERR; for i in 1 2 3; do false; done; echo after'
check "from a subshell"  'trap "exit 9" ERR; ( false ); echo after'
check "from a comsub"    'trap "exit 9" ERR; x=$(false); echo after'
check "from a brace grp" 'trap "exit 9" ERR; { false; }; echo after'
check "nested function"  'trap "exit 9" ERR; g() { false; }; f() { g; }; f; echo after'
check "during wait"      'trap "exit 9" USR1; ( sleep 0.1; kill -USR1 $$ ) & wait; echo after'

# --- ordering rules ---------------------------------------------------------
check "EXIT still fires" 'trap "echo E" EXIT; trap "exit 9" ERR; false'
check "last exit wins"   'trap "exit 7" EXIT; trap "exit 9" ERR; false'
check "exit in EXIT"     'trap "echo E; exit 7" EXIT; true'
check "beats errexit"    'set -e; trap "exit 9" ERR; false; echo after'
check "errexit alone"    'set -e; trap "echo E" ERR; false; echo after'
check "DEBUG pre-empts"  'trap "echo D; exit 3" DEBUG; trap "exit 9" ERR; false'

# --- traps that do NOT exit are unchanged -----------------------------------
check "plain ERR"        'trap "echo E" ERR; false; echo after'
check "plain EXIT"       'trap "echo E" EXIT; true'
check "status survives"  'trap "echo E" ERR; false; echo "rc=$?"'
check "plain DEBUG"      'trap "echo D" DEBUG; echo a'
check "exit 0 from trap" 'trap "exit 0" ERR; false; echo after'
check "trap then normal" 'trap "echo E" ERR; false; true; echo "rc=$?"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
