#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v228: command-not-found error format
# (word order + non-interactive prologue). Runs each fragment as a SCRIPT FILE
# (file mode) on the SAME temp path for both shells, so the `<path>: line N:`
# prologue matches byte-for-byte. Compares stdout+stderr+rc.
#
# Scope: only the spawn-NotFound path (a resolved-but-missing external command,
# including the quoted-empty `''` real-field case). The zero-field command-word
# cases ($empty / $empty arg / $empty >redir) are a separate deferred divergence
# (bash no-ops or promotes; huck errors) and are NOT asserted here.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-cnf.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "missing on line 1"      'nosuch_cmd_xyz'
checkf "missing reports line"   'x=1
: ok
nosuch_cmd_xyz'
checkf "missing then continues" 'nosuch_cmd_xyz
echo after'
checkf "missing with args"      'nosuch_cmd_xyz -a b c'
checkf "quoted-empty command"   "''"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
