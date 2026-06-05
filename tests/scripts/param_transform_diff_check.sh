#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v96: ${var@OP} parameter transforms (M-??).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# @U / @L / @u case transforms (ASCII only — non-ASCII inherits a documented
# Rust to_uppercase Unicode divergence, e.g. ß->SS).
check "upper"        'v=hello; echo "${v@U}"'
check "lower"        'v=HeLLo; echo "${v@L}"'
check "upper first"  'v=hello; echo "${v@u}"'
# @Q shell-quote
check "quote word"   'v=hello; echo "${v@Q}"'
check "quote space"  "v='a b'; echo \"\${v@Q}\""
check "quote squote" "v=\"a'b\"; echo \"\${v@Q}\""
check "quote empty"  'v=; echo "${v@Q}"'
check "quote unset"  'unset v; echo "[${v@Q}]"'
# @E backslash-escape expansion (deterministic escapes only)
check "escape tab"   'v='"'"'a\tb'"'"'; echo "${v@E}"'
check "escape nl"    'v='"'"'a\nb'"'"'; echo "${v@E}"'
check "escape unknown" 'v='"'"'a\qb'"'"'; echo "${v@E}"'
# @P prompt expansion (only \n — \u/\h/\w/\$ vary by user/host/cwd/uid)
check "prompt nl"    'v='"'"'x\ny'"'"'; echo "${v@P}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
