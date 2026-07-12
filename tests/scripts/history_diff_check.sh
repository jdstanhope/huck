#!/usr/bin/env bash
# v284: byte-identical bash<->huck for `history N` (#7) and -d/-w/-r/-a (#6).
# File-arg execution (L-27: huck history-expands piped stdin). HISTFILE=/dev/null
# isolates; history is populated with `history -r <fixture>` (works non-interactively).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'cmd-one\ncmd-two\ncmd-three\ncmd-four\n' > "$WORK/fix"
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
check() {
    local label="$1" frag="$2" b h
    printf '%s\n' "$frag" > "$WORK/frag.sh"
    b=$(cd "$WORK" && bash ./frag.sh 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && "$HUCK_BIN" ./frag.sh 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
POP='HISTFILE=/dev/null; history -c; history -r fix;'
check "list all format"   "$POP history"
check "history 2"         "$POP history 2"
check "history 0"         "$POP history 0"
check "history 99"        "$POP history 99"
check "delete single"     "$POP history -d 2; history"
check "delete negative"   "$POP history -d -1; history"
check "delete range"      "$POP history -d 2-3; history"
check "delete oob err"    "$POP history -d 9; echo rc=\$?"
check "delete nonnum err" "$POP history -d abc; echo rc=\$?"
check "read append"       "$POP history -r fix; history"
check "write file"        "$POP history -w out; cat out"
check "append after read" "$POP : > ap; history -a ap; echo \"ap=[\$(cat ap)]\""
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
