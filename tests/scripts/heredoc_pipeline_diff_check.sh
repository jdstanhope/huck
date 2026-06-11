#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v134: heredoc/herestring bodies fed by a
# forked writer never deadlock (M-120). timeout-guarded so a regression FAILS.
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
BIG='V=$(printf "x%.0s" $(seq 1 200000))'
check "compound large"        "$BIG"$'\n{ wc -c; } << E\n$V\nE'
check "compound awk pipe"     "$BIG"$'\n{ command awk "{print}" | wc -l; } << E\n$V\nE'
check "pipeline large"        "$BIG"$'\ncat << E | wc -c\n$V\nE'
check "captured single large" "$BIG"$'\nr=$(cat << E\n$V\nE\n); echo ${#r}'
check "herestring compound"   "$BIG"$'\n{ wc -c; } <<< "$V"'
check "small compound"        $'{ cat; } << E\nhi\nE'
check "small pipeline"        $'cat << E | wc -c\nhi\nE'
check "pipestatus heredoc"    $'false << E | true\nx\nE\necho "${PIPESTATUS[*]}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
