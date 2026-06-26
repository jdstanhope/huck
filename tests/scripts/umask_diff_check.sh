#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v230 umask. File mode on the SAME temp
# path for both shells so the error prologue (`<src>: line N: umask: …`) matches.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-umask.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "octal print"      'umask 022; umask'
checkf "symbolic print"   'umask 022; umask -S'
checkf "posix print"      'umask 022; umask -p'
checkf "posix symbolic"   'umask 002; umask -p -S'
checkf "set symbolic"     'umask -S u=rwx,g=rwx,o=rx; umask'
checkf "octal range err"  'umask 09'
checkf "sym char err"     'umask g=u'
checkf "sym op err"       'umask u:rwx'
checkf "sym colon char"   'umask -S u=rwx:g=rwx,o=rx'
checkf "invalid option"   'umask -i'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
