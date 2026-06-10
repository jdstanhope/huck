#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v132: eval/source run with the enclosing
# sink (capture + redirect). Fragments run as FILE-ARGS via -c.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
printf 'echo SOURCED\n' > "$FIX/s.sh"
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "eval capture"        'x=$(eval "echo hi"); echo "[$x]"'
check "eval multi"          'x=$(eval "echo a; echo b"); echo "[$x]"'
check "eval pipe"           'x=$(eval "seq 1 50 | wc -l"); echo "[$x]"'
check "eval redirect"       'eval "echo R" > '"$FIX"'/r; cat '"$FIX"'/r'
check "eval stderr redir"   'eval "echo E 1>&2" 2> '"$FIX"'/e; cat '"$FIX"'/e'
check "eval stdin redir"    'printf "IN\n" > '"$FIX"'/i; x=$(eval "cat" < '"$FIX"'/i); echo "[$x]"'
check "eval top level"      'eval "echo top"'
check "source capture"      'x=$(source '"$FIX"'/s.sh); echo "[$x]"'
check "source top level"    'source '"$FIX"'/s.sh'
check "command eval"        'x=$(command eval "echo c"); echo "[$x]"'
check "nested eval capture" 'x=$(eval "eval \"echo deep\""); echo "[$x]"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
