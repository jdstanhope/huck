#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v102: braced special params ${-}/${?}/${$}/${!} (M-30/v102).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

# Deterministic compare-and-echo fragments only. Never byte-compare raw
# $-/$$/$! values (they differ across shells/runs); each fragment computes a
# self-relative or fixed result.
check "status"           'false; echo "${?}"'
check "status zero"      'true; echo "${?}"'
check "status default"   'true; echo "${?:-na}"'
check "pid eq"           '[ "${$}" = "$$" ] && echo same || echo diff'
check "dash eq"          '[ "${-}" = "$-" ] && echo same || echo diff'
check "dash noe"         'f() { [ "${-#*e}" = "$-" ] && echo no || echo yes; }; f'
check "bgpid empty"      '[ -z "${!}" ] && echo empty || echo set'
check "nvm shape"        'f() { if [ "${-#*e}" != "$-" ]; then echo yes; else echo no; fi; }; f'
check "count regress"    'set -- a b c; echo "${#}"'
check "indirect regress" 'x=hi; r=x; echo "${!r}"'
harness_summary
