#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v91: extglob pathname globbing (M-84a).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
( cd "$FIX"; touch a b ab aab abc cd xy .hidden .ab; mkdir dir1 dir2
  touch dir1/foo.txt dir1/bar.log dir2/foo.txt )
check() {
    local label="$1" frag="$2" b h
    b=$(printf 'cd %q\nshopt -s extglob\n%s\n' "$FIX" "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf 'cd %q\nshopt -s extglob\n%s\n' "$FIX" "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    compare "$label" "$b" "$h"
}
# NOTE: pathname extglob output is sorted + deterministic, so byte-diffable.
# `**` globstar is a separate (unsupported) shopt and is not exercised.
check "plus"        'echo +(a|b)'
check "at"          'echo @(a|cd)'
check "star"        'echo *(a)'
check "negation"    'echo !(a|ab)'
check "class"       'echo +([a-c])'
check "explicit dot" 'echo .+(ab)'
check "multi-comp"  'echo dir*/+(foo|bar).txt'
check "dirs"        'echo @(dir1|dir2)'
check "no-match lit" 'echo zzz+(q)'
check "compose star" 'echo +(a|b)*'
harness_summary
