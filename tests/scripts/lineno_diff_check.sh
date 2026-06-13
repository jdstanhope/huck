#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v152: LINENO.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check_c() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check_file() {
    local label="$1" body="$2" f b h
    f=$(mktemp); printf '%s' "$body" > "$f"
    b=$(bash "$f" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" "$f" 2>&1; echo "rc=$?")
    rm -f "$f"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# sourced case: write a sub-script to a temp file and source it by PATH (deterministic).
check_sourced() {
    local label="$1" main="$2" sub="$3" mf sf b h
    sf=$(mktemp); printf '%s' "$sub" > "$sf"
    mf=$(mktemp); printf "$main" "$sf" > "$mf"   # %s in main is replaced with the sub path
    b=$(bash "$mf" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" "$mf" 2>&1; echo "rc=$?")
    rm -f "$sf" "$mf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check_c "consecutive"  $'echo $LINENO\necho $LINENO\necho $LINENO'
check_c "in function"  $'f(){\n  echo $LINENO\n}\necho before $LINENO\nf'
check_c "if cond+body" $'if [ $LINENO -ge 0 ]; then echo $LINENO; fi'
check_c "while body"   $'i=0\nwhile [ $i -lt 1 ]; do echo $LINENO; i=1; done'
check_c "nested func"  $'g(){ echo g$LINENO; }\nf(){\n  g\n  echo f$LINENO\n}\nf'
check_file "script"    $'echo $LINENO\nf(){ echo $LINENO; }\nf\necho $LINENO\n'
check_sourced "sourced" $'echo main $LINENO\nsource %s\necho after $LINENO\n' $'echo sub $LINENO\necho sub2 $LINENO\n'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
