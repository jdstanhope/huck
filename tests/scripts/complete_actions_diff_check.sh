#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v88: complete/compgen actions (M-36a).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
# NOTE: only `setopt`/`shopt` action generation + registration-rc are byte-diffed.
# `builtin`/`keyword`/`helptopic`/`command`/`file`/`variable`/`signal`/`job` are
# NOT byte-diffed: their candidate SETS differ between huck and bash (different
# builtin tables / env / PATH / platform signal set) or are volatile. Those are
# membership-tested in tests/complete_actions_integration.rs instead.
check "compgen setopt (all)"   'compgen -A setopt'
check "compgen setopt e"       'compgen -A setopt e'
check "compgen shopt (all)"    'compgen -A shopt'
check "compgen shopt null"     'compgen -A shopt null'
check "register -u rc"         'complete -u cmd; echo rc=$?'
check "register -A stopped rc" 'complete -A stopped cmd; echo rc=$?'
check "register -ev rc"        'complete -ev cmd; echo rc=$?'
check "register -A setopt rc"  'complete -A setopt cmd; echo rc=$?'

# --- #528: compgen's exit status. bash's `build_actions` FAILS when its getopt
#     loop consumed no option at all, and `compgen_builtin` maps that failure
#     back to SUCCESS without generating anything — so "no options" is rc 0
#     whatever words follow, while "options but no matches" is the ordinary
#     rc 1. A bare `--` is not an option, and `+z` is a plain word (#515).
check "compgen bare rc"        'compgen; echo rc=$?'
check "compgen word only rc"   'compgen zzzzznope; echo rc=$?'
check "compgen empty word rc"  'compgen ""; echo rc=$?'
check "compgen plus word rc"   'compgen +z; echo rc=$?'
check "compgen plus-o word rc" 'compgen +o zzzzznope; echo rc=$?'
check "compgen many words rc"  'compgen a b c; echo rc=$?'
check "compgen ddash word rc"  'compgen -- zzzzznope; echo rc=$?'
check "compgen ddash opt rc"   'compgen -- -W abc; echo rc=$?'
check "compgen ddash ddash rc" 'compgen -- --; echo rc=$?'
# Options present: unchanged, and still rc 1 when nothing matches.
check "compgen -W no match rc" 'compgen -W "a b" zzzzznope; echo rc=$?'
check "compgen -W match rc"    'compgen -W "a b" a; echo rc=$?'
check "compgen -o only rc"     'compgen -o nospace zzzzznope; echo rc=$?'
check "compgen -P only rc"     'compgen -P pre; echo rc=$?'
check "compgen -A fn none rc"  'compgen -A function zzzzznope; echo rc=$?'
check "compgen -X filtered rc" 'compgen -X "*" -W a a; echo rc=$?'
# `compgen -q` (invalid option, rc 2) is deliberately NOT a row here: this
# harness does not normalize the program-name prefix, so the diagnostic line
# would differ on the `bash:` vs `<path>/huck:` prefix alone. The status
# itself was verified equal (2) by hand.
harness_summary
