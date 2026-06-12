#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v150: process substitution <(...) / >(...).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
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
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
