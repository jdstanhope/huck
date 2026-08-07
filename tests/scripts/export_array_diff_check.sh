#!/usr/bin/env bash
# v282: byte-identical bash<->huck for exported arrays.
#   #82 — `export a=(...)` assigns the indexed array + marks it exported
#         (declare -ax), rc 0 (huck used to error "cannot export arrays").
#   #28 — an exported array is NOT inherited by a child process (bash puts no
#         array in the environment); an exported scalar IS. `printenv` is an
#         ordinary external child, so the same fragment runs under both shells.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    compare "$label" "$b" "$h"
}
check "export array assigns"     'export a=(1 2 3); declare -p a'
check "export array rc"          'export a=(1 2 3); echo "rc=$?"'
check "export existing array"    'a=(x y); export a; declare -p a'
check "export array append"      'a=(1 2 3); export a+=(4 5); declare -p a'
check "array not in child env"   'export a=(x y z); printenv a; echo "rc=$?"'
check "scalar IS in child env"   'export s=hi; printenv s; echo "rc=$?"'
# #28: an inline-prefix scalar over an array IS exported to the child as that
# scalar (unlike a persistent exported array); the parent array is restored.
check "inline scalar over array" 'FOO=(o d m); FOO=inner printenv FOO; echo "rc=$? after:${FOO[*]}"'
check "inline nested restore"    'FOO=(a b c); f(){ FOO=in2 printenv FOO; echo "inrc=$?"; printenv FOO; echo "aftrc=$?"; }; FOO=out f; echo "final:${FOO[*]}"'
# #28: a redirection-only `exec` with an inline scalar over an array must not
# leak that scalar into the child env of subsequent commands.
check "inline over array + exec" 'FOO=(a b c); FOO=inner exec 3>/dev/null; printenv FOO; echo "rc=$?"'
harness_summary
