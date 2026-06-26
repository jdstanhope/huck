#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v230 ulimit. ENV-INDEPENDENT cases only
# (round-trips of values we set in-script, and error forms). NOT `-a` absolutes.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ulimit.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "nofile roundtrip"  'ulimit -n 64; ulimit -n'
checkf "core soft set"     'ulimit -c unlimited; ulimit -c -S -- 1000; ulimit -c'
checkf "unlimited query"   'ulimit -c unlimited; ulimit -c'
checkf "invalid number"    'ulimit -n abc'
checkf "invalid option"    'ulimit -Z'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
