#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v150: process substitution <(...) / >(...).
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    compare "$label" "$b" "$h"
}
check "cat input"        'cat <(echo hi)'
check "two inputs"       'cat <(echo a) <(echo b)'
check "diff"             'diff <(printf "a\nb\n") <(printf "a\nc\n"); echo "rc=$?"'
check "comm"             'comm -12 <(printf "a\nb\nc\n") <(printf "b\nc\nd\n")'
check "redirect source"  'wc -c < <(printf hello)'
check "while read"       'while read x; do echo "[$x]"; done < <(seq 3)'
check "output sub"       'f=$(mktemp); echo hello > >(cat > "$f"); wait; cat "$f"; rm "$f"'
check "nested"           'cat <(cat <(echo deep))'
check "quoted literal"   'echo "<(echo hi)"'
check "paste"            'paste <(seq 2) <(seq 2)'
harness_summary
