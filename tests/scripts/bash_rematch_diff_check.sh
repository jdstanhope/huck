#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v122: BASH_REMATCH population after
# [[ =~ ]] (M-14 sub-feature). File-arg execution (L-27).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>&1; echo "EXIT:$?")
    rm -f "$tf"
    compare "$label" "$b" "$h"
}

check "whole+groups"    '[[ abcdef =~ b(c)(d) ]]; echo "n=${#BASH_REMATCH[@]} [${BASH_REMATCH[0]}][${BASH_REMATCH[1]}][${BASH_REMATCH[2]}]"'
check "no-match clears"  'BASH_REMATCH=(stale x y); [[ xyz =~ nomatch ]]; echo "rc=$? n=${#BASH_REMATCH[@]}"'
check "nonpart group"   '[[ ab =~ (a)|(b) ]]; echo "[${BASH_REMATCH[1]}][${BASH_REMATCH[2]}]"'
check "substring"       '[[ foobar =~ o+ ]]; echo "[${BASH_REMATCH[0]}]"'
check "quoted regex"    '[[ "a.b" =~ "a.b" ]]; echo "rc=$? [${BASH_REMATCH[0]}]"'
check "anchored"        '[[ hello =~ ^h.*o$ ]]; echo "[${BASH_REMATCH[0]}]"'
check "digits group"    '[[ "v1.2.3" =~ ([0-9]+)\.([0-9]+) ]]; echo "[${BASH_REMATCH[1]}][${BASH_REMATCH[2]}]"'
check "longopt extract"  'for w in --all -x --almost-all; do [[ $w =~ (--[a-z-]+) ]] && printf "%s\n" "${BASH_REMATCH[1]}"; done'
check "rematch indices"  '[[ abcdef =~ b(c)(d) ]]; echo "${!BASH_REMATCH[@]}"'

harness_summary
