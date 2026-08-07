#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v84: ${...} operands parse as words.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
# NOTE: the two M-15b cases (a `${...}` inside outer double-quotes whose operand
# contains single quotes, or a backslash-escaped char) are pre-existing,
# documented-deferred divergences (see bash-divergences.md M-15b) — intentionally
# omitted here so this harness stays byte-identical to bash.
check "alt parens+expansion"  'x=v; echo "[${x:+($x)}]"'
check "alt unset"             'unset y; echo "[${y:+($y)}]"'
check "default metachars"     'unset y; echo "[${y:-(a|b;c)}]"'
check "default unquoted split" 'unset y; for w in ${y:-a b c}; do printf "%s|" "$w"; done; echo'
check "default quoted one"    'unset y; for w in "${y:-a b c}"; do printf "%s|" "$w"; done; echo'
check "metachars in dquote"   'unset y; echo "[${y:-|;()}]"'
check "debian PS1 operand"    'debian_chroot=; PS1="${debian_chroot:+($debian_chroot)}x"; echo "$PS1"'
check "subst pattern parens"  'v="a(b)c"; echo "${v/(b)/X}"'
harness_summary
