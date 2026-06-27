#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v231 A+B: source CWD/sourcepath fallback
# + device-file/fifo acceptance. File mode on the SAME temp path for both shells.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-srcdev.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
checkf_pipe() {
    local label="$1" body="$2" feed="$3" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-srcdev.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(printf '%s\n' "$feed" | bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$feed" | "$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf       "dev null"        '. /dev/null; echo "rc=$?"'
checkf_pipe  "dev stdin"       '. /dev/stdin; echo end' 'echo PIPED'
checkf       "fifo source"     'f=$(mktemp -u "${TMPDIR:-/tmp}/huck-fifo.XXXXXX"); mkfifo "$f"; { echo "echo FIFO_OK" > "$f" & }; . "$f"; echo "rc=$?"; rm -f "$f"'
checkf       "missing"         '. /no/such_xyz_v231; echo "rc=$?"'
checkf       "directory"       '. /etc; echo "rc=$?"'
checkf       "sourcepath off"  'shopt -u sourcepath; d=$(mktemp -d "${TMPDIR:-/tmp}/huck-sd.XXXXXX"); echo "set -- m n o p" > "$d/x.sub"; cd "$d"; . x.sub; echo "$@"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
