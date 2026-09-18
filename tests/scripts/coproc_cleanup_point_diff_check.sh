#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #185: WHEN a coproc that has exited
# is torn down. bash marks it dead when the child is reaped, but closes its
# fds and unsets NAME/NAME_PID only at the next cleanup point (`coproc_reap`
# runs inside `cleanup_dead_jobs`): the end of a foreground wait, `jobs`, a
# loop iteration, `wait`, or a new input line. Between those a single-shot
# coproc's output is still readable, and a `$( )` is NOT a cleanup point.
#
# `jobs` output itself is not compared (its command column differs: bash
# prints the coproc body) — rows send it to /dev/null and read the state after.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
norm() { sed -E 's|^[^:]*: line |PROG: line |'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash --norc --noprofile -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [-c]" "$b" "$h"
    printf '%s\n' "$frag" >"$tmp/f.sh"
    b=$(bash --norc --noprofile "$tmp/f.sh" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    h=$("$HUCK_BIN" "$tmp/f.sh" 2>&1 | norm; echo "rc=${PIPESTATUS[0]}")
    compare "$label [file]" "$b" "$h"
}
one='coproc X { read l; echo "e:$l"; }; echo yo >&"${X[1]}"; '

# --- not cleanup points: builtins, a $( ), a new job ------------------------
check "read straight back"     "$one"'read r <&"${X[0]}"; echo "[$r]"'
check "builtins between"       "$one"':; :; x=1; read r <&"${X[0]}"; echo "[$r]"'
check "comsub between"         "$one"'x=$(sleep 0.3); read r <&"${X[0]}"; echo "[$r]"'
# (Bodies that live a moment: a coproc that exits before bash has even set its
# variables is disposed on the spot — a bash-side race, not a cleanup rule.)
check "PID survives a comsub"  'coproc X { sleep 0.1; }; x=$(sleep 0.3); echo "[${X_PID:+set}] [${X[0]:+set}]"'

# --- cleanup points: fg wait, jobs, loop, wait, next line -------------------
check "fg wait disposes"       "$one"'sleep 0.3; read r <&"${X[0]}"; echo "[$r]"'
check "jobs disposes"          "$one"'x=$(sleep 0.3); jobs >/dev/null; read r <&"${X[0]}"; echo "[$r] [${X_PID:-unset}]"'
check "loop disposes"          "$one"'x=$(sleep 0.3); for i in 1; do :; done; echo "[${X_PID:-unset}]"'
check "wait \$PID disposes"    'coproc X { sleep 0.1; }; x=$(sleep 0.3); wait $X_PID; echo "rc=$? [${X[0]:-unset}] [${X_PID:-unset}]"'
check "bare wait disposes"     'coproc X { sleep 0.1; }; x=$(sleep 0.3); wait; echo "[${X_PID:-unset}]"'
check "next line disposes"     $'coproc X { sleep 0.1; }\nx=$(sleep 0.3)\necho "[${X_PID:-unset}]"'
check "same line keeps it"     $'coproc X { sleep 0.1; }; x=$(sleep 0.3); echo "[${X_PID:+set}]"'

# --- a live coproc is never touched ----------------------------------------
check "live across jobs"       'coproc X { while read l; do echo "e:$l"; done; }; echo a >&"${X[1]}"; jobs >/dev/null; for i in 1; do :; done; read r <&"${X[0]}"; echo "[$r] [${X_PID:+set}]"; kill $X_PID; wait $X_PID 2>/dev/null; echo "[${X_PID:-unset}]"'

# --- the #185 delay table ---------------------------------------------------
for d in 0.005 0.05; do
    check "delay $d" "$one""sleep $d; "'read r <&"${X[0]}"; echo "[$r]"'
done

harness_summary
