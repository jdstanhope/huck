#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v142: the `builtin` builtin.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
check "builtin echo"        'builtin echo hi'
check "builtin cd"          'builtin cd /tmp; pwd'
check "builtin alone"       'builtin; echo "rc=$?"'
check "cd wrapper"          'cd(){ builtin cd "$@"; }; cd /tmp; pwd'
check "bypass cd fn"        'cd(){ echo SHADOW; }; builtin cd /tmp; pwd'
check "builtin local"       'f(){ builtin local x=5; echo "$x"; }; f'
check "builtin pwd"         'builtin cd /tmp; builtin pwd'
check "command -v builtin"  'command -v builtin'
check "builtin builtin local" 'f(){ builtin builtin local x=5; echo "$x"; }; f'
check "builtin command cd"  'builtin command cd /tmp; pwd'
check "command builtin cd"  'command builtin cd /tmp; pwd'
harness_summary
