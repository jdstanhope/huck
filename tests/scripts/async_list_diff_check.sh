#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v98: & async list separator (M-??).
# All fragments are DETERMINISTIC: background writers append to a file, then
# `wait`, then read with sort/cat/wc -l. Never rely on interleave timing.
# Never byte-compare $! (pid varies). Never use `&;` (both shells reject it).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT

check "amp sep"        ": > '$FIX/a'; echo x >> '$FIX/a' & wait; echo y >> '$FIX/a'; sort '$FIX/a'"
check "amp in for"     ": > '$FIX/b'; for i in 1 2 3; do echo \$i >> '$FIX/b' & done; wait; sort '$FIX/b'"
check "group bg true"  ": > '$FIX/c'; true && echo g >> '$FIX/c' & wait; cat '$FIX/c'"
check "group bg false" ": > '$FIX/d'; false && echo no >> '$FIX/d' & wait; cat '$FIX/d'"
check "trailing status" "false & echo \$?; wait"
check "semi/and/or"    "true && echo y; false || echo n; echo a; echo b"
check "brace amp"      ": > '$FIX/e'; { echo a >> '$FIX/e' & wait; echo b >> '$FIX/e'; }; sort '$FIX/e'"
check "mixed grouping" ": > '$FIX/f'; true && echo g1 >> '$FIX/f' & echo g2 >> '$FIX/f' || echo g3 >> '$FIX/f'; wait; sort '$FIX/f'"
check "ctrl flow after bg" ": > '$FIX/g'; for i in 1 2 3; do echo z >> '$FIX/g' & if [ \$i = 2 ]; then break; fi; done; wait; wc -l < '$FIX/g'"
check "set -e bg exempt" "set -e; false & wait; echo survived"
harness_summary
