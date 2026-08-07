#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v148: prompt-string variables and the
# environment. bash does NOT import PS1/PS2 from the environment (a non-interactive
# shell leaves them empty), but DOES import PS0/PS4. huck must match.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
# check LABEL  ENV-ASSIGNMENTS  FRAGMENT  — runs `env <assigns> <shell> -c <frag>`.
check() {
    local label="$1" assigns="$2" frag="$3" b h
    b=$(env $assigns bash --norc -c "$frag" 2>&1; echo "rc=$?")
    h=$(env $assigns "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
check "PS1 not imported from env"   "PS1=ENVPS1" 'printf "[%s]" "$PS1"'
check "PS2 not imported from env"   "PS2=ENVPS2" 'printf "[%s]" "$PS2"'
check "PS0 IS imported from env"    "PS0=ENVPS0" 'printf "[%s]" "$PS0"'
check "PS4 IS imported from env"    "PS4=ENVPS4" 'printf "[%s]" "$PS4"'
check "inherited PS1 cmdsub inert"  'PS1=$(echo hi)' 'printf "[%s]" "$PS1"'
check "PS1 assignable after skip"   "PS1=ENVPS1" 'PS1="set>"; printf "[%s]" "$PS1"'
check "normal var still imported"   "FOO=bar"    'printf "[%s]" "$FOO"'
check "PS1 unset is empty"          ""           'printf "[%s]" "${PS1:-}"'
harness_summary
