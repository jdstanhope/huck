#!/usr/bin/env bash
# v286 (#121): the move-fd redirect `[n]<&digit-` / `[n]>&digit-` (dup then close
# the source fd) must match bash byte-for-byte. Error prefixes are normalized so
# only the message tail + rc are compared.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'A\nB\n' > "$WORK/f"
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
check() {
    local label="$1" frag="$2" b h
    b=$(cd "$WORK" && bash -c "$frag" 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && "$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# The #121 repro: move fd5 (a file) onto stdin, then read stdin.
check "move file to stdin"   'exec 5<f; exec 0<&5-; cat'
check "move+read loop"       'exec 5<f; exec 0<&5-; while read l; do echo "got:$l"; done; echo DONE'
# Output move in a subshell, then inspect the file.
check "output move"          '( exec 5>o; exec 1>&5-; echo hi ); cat o'
# After a move the source fd is closed: reading it fails.
check "source fd closed"     'exec 5<f; exec 0<&5-; read x <&5; echo "rc=$?"'
# Degenerate N>&N- (target == source) closes fd N.
check "degenerate NgtN"      '( exec 5>o; exec 5>&5-; echo x >&5 ) 2>/dev/null; echo "rc=$?"'
# Bad source fd → error (compare normalized message + rc).
check "bad source fd"        '( exec 7<&9- ) 2>&1; echo "rc=$?"'
# Command-scoped move restores the target afterward.
check "command-scoped move"  'exec 5<f; cat 0<&5-; echo "after:$(echo restored)"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
