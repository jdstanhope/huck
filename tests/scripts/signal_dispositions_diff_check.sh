#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #478/#766: which signals the shell and
# its children have IGNORED, read straight from /proc.
#
# bash ignores SIGQUIT in every shell (`initialize_shell_signals`), hands a
# child the defaults back (`restore_original_signals`), and then — for an
# asynchronous unit started with job control off — ignores INT and QUIT again
# (`setup_async_signals`). Only the command that IS the async unit keeps that:
# `cmd &`, each stage of `a | b &`; anything inside a compound async body
# (`{ }`, `( )`, `if`, a function, a multi-command coproc) gets the defaults.
#
# `trap '' SIG` propagation to children is a separate, open gap and is not
# rowed here.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
G='grep SigIgn /proc/self/status'
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1; echo "rc=$?")
    h=$(ulimit -c 0; "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}

# --- the shell itself survives QUIT (#478) ----------------------------------
check "QUIT to the shell"          'kill -QUIT $$; sleep 0.2; echo alive'
check "QUIT after trap reset"      'trap "echo t" QUIT; kill -QUIT $$; sleep 0.1; trap - QUIT; kill -QUIT $$; sleep 0.2; echo alive'
check "QUIT is trappable"          'trap "echo t" QUIT; kill -QUIT $$; sleep 0.1; echo alive'

# --- children get the defaults back -----------------------------------------
check "foreground child"           "$G"
check "subshell"                   "( $G )"
check "command substitution"       "x=\$($G); echo \$x"
check "pipeline stage"             "sleep 0.05 | $G"
check "process substitution"       "cat <($G)"
check "exec"                       "exec $G"

# --- the async unit ignores INT and QUIT without job control (#766) ---------
check "cmd &"                      "$G & wait"
check "pipeline &"                 "sleep 0.05 | $G & wait"
check "pipeline & first stage"     "$G | cat & wait"
check "cmd & under set -m"         "set -m; $G & wait"
check "second of two"              "$G; $G & wait"
check "INT does not kill cmd &"    'sleep 0.3 & p=$!; sleep 0.1; kill -INT $p; wait $p; echo rc=$?'
check "QUIT does not kill cmd &"   'sleep 0.3 & p=$!; sleep 0.1; kill -QUIT $p; wait $p; echo rc=$?'
check "INT kills cmd & under -m"   'set -m; sleep 0.3 & p=$!; sleep 0.1; kill -INT $p; wait $p 2>/dev/null; echo rc=$?'

# --- inside a compound async body: defaults again ---------------------------
check "{ } &"                      "{ $G; } & wait"
check "( ) &"                      "( $G ) & wait"
check "if &"                       "if true; then $G; fi & wait"
check "function &"                 "f() { $G; }; f & wait"
check "{ a; b; } &"                "{ $G; $G; } & wait"
check "coproc, multi-command"      "coproc X { sleep 0.05; $G; }; cat <&\"\${X[0]}\""
check "coproc, pipeline body"      "coproc X { $G | cat; }; cat <&\"\${X[0]}\""
check "sh -c &"                    "/bin/sh -c '$G' & wait"

harness_summary
