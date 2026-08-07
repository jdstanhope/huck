#!/usr/bin/env bash
# v147: export -f — byte-comparable building blocks (env-key shape + declare -fx trailer + child run).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
check "BASH_FUNC env key"   'f(){ echo x; }; export -f f; env | grep -o "BASH_FUNC_f%%" | head -1'
check "declare -fx trailer" 'f(){ echo x; }; export -f f; export -f | grep "^declare -fx f$"'
check "not a function rc"   'export -f nope 2>/dev/null; echo "rc=$?"'
check "export -p no funcs"  'f(){ echo x; }; export -f f; export -p | grep -c "BASH_FUNC" || true'
check "child runs function" 'f(){ echo HELLO; }; export -f f; bash -c f'
harness_summary
