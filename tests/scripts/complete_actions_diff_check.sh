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
harness_summary
