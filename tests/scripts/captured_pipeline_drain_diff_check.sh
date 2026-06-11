#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v133: a captured pipeline larger than the
# pipe buffer must not deadlock (M-119). Each fragment is wrapped in `timeout` so a
# regression shows as a FAIL (non-zero exit / truncated output), not a hung harness.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(timeout 15 bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$(timeout 15 "$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "large captured pipe"   'x=$(seq 1 500000 | cat); echo ${#x}'
check "three-stage captured"  'x=$(seq 1 200000 | cat | cat); echo ${#x}'
check "small captured pipe"   'x=$(seq 1 1000 | cat); echo ${#x}'
check "large producer small"  'x=$(seq 1 500000 | wc -l); echo "[$x]"'
check "pipe tr filter large"  'x=$(seq 1 500000 | tr -d "\n" | wc -c); echo "[$x]"'
check "non-capture pipe"      'seq 1 100 | wc -l'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
