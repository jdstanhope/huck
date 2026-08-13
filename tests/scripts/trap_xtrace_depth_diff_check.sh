#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the xtrace DEPTH of a trap action
# (#486).
#
# bash traces a trap action's commands one level deeper than the command that
# triggered it — the same indirection bump a command substitution gets:
#
#     set -x; trap "echo D" DEBUG; true
#     + trap 'echo D' DEBUG
#     ++ echo D                          <- huck printed `+ echo D`
#     D
#     + true
#
# With the action at the caller's depth there was no way to tell a trap action's
# commands from the traced command itself, which is also why
# `debug_cond_arith_diff_check.sh` proves DEBUG fire ORDER by mutating a
# variable instead of by reading the trace.
#
# EXIT is the exception, and it is bash's, not an oversight: its action traces
# at the CALLER's depth, because it runs on the shell's termination path rather
# than nested inside anything. Both EXIT rows are here so the increment cannot
# be applied to it by accident.
#
# The nesting composes: a command substitution inside a DEBUG action reaches
# `+++`, and a function called from an action traces its body at the action's
# depth.
#
# NOT here: an assignment with an EMPTY value (`x=$(true)`), which huck traces
# as `x=''` where bash prints a bare `x=` — #614, unrelated to depth.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- one level deeper: DEBUG, ERR, RETURN, and a real signal ---
check "debug action"       'set -x; trap "echo D" DEBUG; true'
check "debug two commands" 'set -x; trap "echo A; echo B" DEBUG; true'
check "err action"         'set -x; trap "echo E" ERR; false'
check "err in a function"  'set -x; trap "echo E" ERR; f(){ false; }; f'
check "return action"      'set -T; set -x; trap "echo R" RETURN; f(){ :; }; f'
check "return set inside"  'set -x; f(){ trap "echo R" RETURN; :; }; f'
# The pid in `kill -USR1 <pid>` is each shell's own, so that one line is
# normalised; every other line, including the traced action, is compared as-is.
check_pid() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1 \
        | sed -E 's/-USR1 [0-9]+/-USR1 PID/'; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1 \
        | sed -E 's/-USR1 [0-9]+/-USR1 PID/'; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
check_pid "signal action"  'set -x; trap "echo U" USR1; kill -USR1 $$; sleep 0.2'

# --- the nesting composes ---
check "comsub in an action" 'set -x; trap "echo \$(echo sub)" DEBUG; true'
check "function from action" 'f(){ echo in; }; set -x; trap "f" DEBUG; true'
check "function defined in action" 'set -x; trap "f2(){ echo inner; }; f2" DEBUG; true'
check "action disarms itself" 'set -x; trap "trap - DEBUG; echo D" DEBUG; true; true'
check "debug before a call" 'set -x; trap "echo D" DEBUG; f(){ :; }; f'
check "action in a subshell" 'set -x; trap "echo D" DEBUG; (true)'

# --- EXIT keeps the caller's depth ---
check "exit action"        'set -x; trap "echo X" EXIT; true'
check "exit via exit"      'set -x; trap "echo X" EXIT; exit 0'
check "exit with a comsub" 'set -x; trap "echo \$(echo sub)" EXIT; true'
check "exit and debug"     'set -x; trap "echo X" EXIT; trap "echo D" DEBUG; true'

# --- controls: the depths that were already right ---
check "plain comsub"       'set -x; x=$(echo hi); echo "$x"'
check "nested comsub"      'set -x; x=$(echo $(echo hi)); echo "$x"'
check "function body"      'set -x; f(){ echo in; }; f'
check "no trap at all"     'set -x; true; false; echo done'
check "trap without xtrace" 'trap "echo D" DEBUG; true; echo done'

harness_summary
