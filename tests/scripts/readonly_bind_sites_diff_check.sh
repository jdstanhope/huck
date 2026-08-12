#!/usr/bin/env bash
# Byte-identical bash<->huck harness for the three sites that BIND a shell-chosen
# integer to a user-named variable (#224): `wait -n -p VAR`, `{VAR}>file`, and a
# coprocess's `NAME_PID`.
#
# Each one wrote straight through the setter leaf, so a READONLY name was
# silently overwritten. bash refuses all three — with three different messages,
# three different statuses and three different amounts of work still done:
#
#   wait -p VAR   `wait: VAR: cannot unset: readonly variable`, status 1, and
#                 NOTHING is waited for — the check is up front (measured: the
#                 error is instant, not after the child finishes). The wording
#                 is its own because `wait` unsets before assigning.
#   {VAR}>file    TWO lines, `VAR: readonly variable` then
#                 `VAR: cannot assign fd to variable`, the redirect FAILS and
#                 the command does not run.
#   NAME_PID      `NAME_PID: readonly variable`, status 0 — the coprocess still
#                 starts, only the assignment is refused.
#
# PIDs are normalised away (`=NNNN` -> `=PID`): the control rows assign a real
# pid, which cannot match between two shells.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

BASH_BIN="${BASH_BIN:-bash}"

norm() { sed -E 's/=[0-9]{3,}/=PID/g'; }

# DRIVER: `-c` with an explicit $0 so the prologue matches byte for byte.
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 10 "$BASH_BIN" --norc --noprofile -c "$frag" huck5 2>&1 | norm; echo "EXIT:$?")
    h=$(timeout 10 "$HUCK_BIN" -c "$frag" huck5 2>&1 | norm; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# --- wait -p: refused before anything is waited for ---
check "wait -n -p readonly"    'readonly FOO=1; sleep 0.1 & wait -n -p FOO; echo "rc=$? FOO=$FOO"'
check "wait -n -p no jobs"     'readonly FOO=1; wait -n -p FOO; echo "rc=$? FOO=$FOO"'
check "wait -p pid readonly"   'readonly FOO=1; sleep 0.05 & wait -p FOO $!; echo "rc=$? FOO=$FOO"'
check "wait -p empty readonly" 'readonly FOO=; wait -n -p FOO; echo "rc=$? FOO=[$FOO]"'

# --- {var} fd: two lines, and the command does not run ---
check "exec {var} readonly"    'readonly FOO=1; exec {FOO}>/dev/null; echo "rc=$? FOO=$FOO"'
check "command {var} readonly" 'readonly FOO=1; echo hi {FOO}>/dev/null; echo "rc=$? FOO=$FOO"'
check "group {var} readonly"   'readonly FOO=1; { echo hi; } {FOO}>/dev/null; echo "rc=$?"'
check "function {var} readonly" 'readonly FOO=1; f(){ echo hi; }; f {FOO}>/dev/null; echo "rc=$?"'
check "{var} input readonly"   'readonly FOO=1; read v {FOO}</dev/null; echo "rc=$?"'

# --- NAME_PID: reported, but the coprocess still runs ---
check "coproc _PID readonly"   'readonly c_PID=1; coproc c { sleep 0.05; }; echo "rc=$? pid=$c_PID"; wait'

# --- controls: none of this fires on an ordinary variable ---
check "wait -p plain"          'FOO=1; sleep 0.05 & wait -n -p FOO; echo "rc=$?"'
check "exec {var} plain"       'exec {FOO}>/dev/null; echo "rc=$? FOO=$FOO"'
check "command {var} plain"    'echo hi {FOO}>/dev/null; echo "rc=$?"'
check "coproc plain"           'coproc c { sleep 0.05; }; echo "rc=$?"; wait'

harness_summary
