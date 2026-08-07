#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v321: nested-"..." backslash handling
# inside a value-family parameter-expansion word under a double-quoted outer
# ${...} (rhs-exp category, #253). A backslash before a non-special char in a
# nested "..." span of a dquoted ${...} word is DROPPED (\p -> p), matching
# bash 5.2.21; $, `, ", \ stay special (kept escaped/literal as usual).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}

check "dq outer nested \\p"       'v=X; printf "<%s>\n" "${v:+a="\p"b}"'
check "dq outer nested \\squote" 'v=X; printf "<%s>\n" "${v:+a="\'"'"'"b}"'
check "dq outer nested \\dollar"  'v=X; printf "<%s>\n" "${v:+a="\$"b}"'
check "dq outer nested \\backsl"  'v=X; printf "<%s>\n" "${v:+a="\\"b}"'
check "dq outer bare \\p"         'v=X; printf "<%s>\n" "${v:+a=\pb}"'
check "unquoted outer nested \\p" 'v=X; printf "<%s>\n" ${v:+a="\p"b}'
check "colon-minus dq nested \\p" 'unset u; printf "<%s>\n" "${u:-a="\p"b}"'
check "colon-eq dq nested \\p"    'unset w; printf "<%s>\n" "${w:=a="\p"b}"'
check "plain dq no param"         'printf "<%s>\n" "A\pB"'
check "multichar nested run"      'v=X; printf "<%s>\n" "${v:+a="x\py\qz"b}"'

harness_summary
