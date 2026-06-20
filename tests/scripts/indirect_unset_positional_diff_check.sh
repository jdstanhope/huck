#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v195: `${!N}` indirect expansion when
# the source positional/var is unset or set-but-empty.
#
# bash's three-way rule for an empty through-value (${!param} where param
# resolves to nothing):
#   - unset POSITIONAL ($1.. beyond $#)  -> expands to empty, rc 0
#     (or "!N: unbound variable" under `set -u`)
#   - set-but-empty source (positional or named) -> ": invalid variable name"
#   - unset NAMED var -> "name: invalid indirect expansion"
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Normalize the diverging error-line prefix: bash uses "bash: line N: ",
# huck uses "huck: " — strip both so the message TEXT is what's compared.
norm() { sed -E 's/^(bash|huck): (line [0-9]+: )?//'; }
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Message-only check: drops the rc line. Used for the `set -u` cases, where
# bash exits 127 but huck's nounset-during-expansion convention is exit 1
# (a pre-existing, general divergence — see `set -u; echo "${unset}"`).
check_msg() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1 | norm)
    h=$("$HUCK_BIN" -c "$frag" 2>&1 | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# --- unset positional -> empty, rc 0 ---
check "unset \$1"          'echo "[${!1}]"'
check "unset \$2"          'set -- a; echo "[${!2}]"'
check "unset \$9"          'echo "[${!9}]"'
check "unset \$10"         'echo "[${!10}]"'
check "two unset pos"      'echo "[${!1}${!2}]"'
check "over-shifted"       'set -- a b c; shift 5 2>/dev/null; echo "[${!1}]"'
check "unset pos in func"  'f(){ echo "[${!1}]"; }; f'
check "unset pos default"  'echo "[${!1:-DEF}]"'
check "unset pos plus"     'echo "[${!2:+SET}]"'
# --- set-but-empty -> ": invalid variable name" ---
check "set-empty pos \$1"  'set -- ""; echo "x${!1}y"'
check "set-empty pos \$2"  'set -- a ""; echo "x${!2}y"'
check "set-empty named"    'x=; echo "x${!x}y"'
# --- unset named -> "invalid indirect expansion" (unchanged) ---
check "unset named"        'unset x; echo "x${!x}y"'
# --- value/transform modifiers on unset positional -> empty, rc 0 ---
check "unset pos #pat"     'echo "[${!1#x}]"'
check "unset pos %pat"     'echo "[${!1%x}]"'
check "unset pos /a/b"     'echo "[${!1/a/b}]"'
check "unset pos :off:len" 'echo "[${!1:0:2}]"'
check "unset pos ^^"       'echo "[${!1^^}]"'
# --- assignment/error modifiers on unset positional -> bash errors (no vars[""] write) ---
check     "unset pos :=VAL"  'echo "[${!1:=VAL}]"'
check     "unset pos =VAL"   'echo "[${!1=VAL}]"'
check_msg "unset pos :?"     'echo "${!1:?}"'
check_msg "unset pos :?boom" 'echo "${!1:?boom}"'
check_msg "unset pos ?"      'echo "${!1?}"'
# ${!1:=VAL} must NOT leave an empty-named var behind (regression guard):
check "no empty-name var"  'echo "[${!1:=VAL}]" 2>/dev/null; printf "after=%s" "$(set 2>/dev/null | grep -c "^=")"'
# --- set -u: unset positional -> "!N: unbound variable" (message-only; see above) ---
check_msg "set -u unset \$1"   'set -u; echo "[${!1}]"'
check_msg "set -u unset \$5"   'set -u; echo "[${!5}]"'
# --- controls: valid indirection still works ---
check "valid via named"    'HOME=x x=HOME; echo "[${!x}]"'
check "valid via pos"      'set -- HOME; echo "[${!1}]"'
check "pos to unset named" 'set -- a; echo "[${!1}]"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
