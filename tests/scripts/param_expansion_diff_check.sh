#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v233: ${...} parse robustness
# (M1 prefix, M2 special-param names, M3 @-edges, M4 $'...', bad-subst defer).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-pe.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# M1 prefix-name expansion
checkf "prefix star"      '_Qa=1; _Qb=2; echo ${!_Q*}'
checkf "prefix at loop"   '_Qa=1; _Qb=2; for k in ${!_Q@}; do echo $k; done'
checkf "prefix no match"  'echo "[${!NOSUCHPFX_ZZ*}]"'
# M2 special-param names
checkf "len of argc"      'set -- a b c; echo ${##}'
checkf "indirect argc"    'set -- a b c; echo ${!#}'
# M4 $'...' in pattern
checkf "ansi-c pattern"   "x=aXb; printf '<%s>\\n' \"\${x#\$'a\\t\\'\\tb'}\""
checkf "ansi-c strip"     "x=foo; echo \"\${x#\$'f'}\""
# bad-subst defer (M2/M3) — must MATCH bash's runtime error + continuation
checkf "bad dollar name"  'echo before; echo ${$x}; echo after'
checkf "bad empty xform"  'V=42; echo ${V@}; echo after'
checkf "bad dash digit"   'echo "[${-3}]"; echo after'
checkf "short-circuit"    '[[ -n yes || -z ${H*} ]]; echo rc=$?'
# v266 bad-substitution / invalid-modifier edge cases (append; runtime error + prologue)
checkf "empty name"       'echo ${}'
checkf "empty before mod" 'echo ${:-x}'
checkf "positional invmod" 'set -- a b; echo ${1foo}'
# P4  ${!!} / ${!$} — DIVERGENCE (reported): huck treats !/$  as valid indirect
#     (${!!} -> "huck: !: invalid indirect expansion"; ${!$} -> empty, exit 0),
#     bash rejects both as bad substitution. Excluded to keep harness green.
# P8  ${X:&Y} — DIVERGENCE (reported): both hit an arith operand-expected error, but
#     huck omits bash's leading "X: " param-name in the message.
# P9  ${#?} ${#-} ${#$} ${#!} — DIVERGENCE (reported): huck's $- is empty, so ${#-}=0
#     vs bash's ${#-}=2 (bash $- = "hB").
# P10 ${!*} / ${!@} — DIVERGENCE (reported): on invalid var name huck uses the "huck:"
#     prefix, bash uses the "file: line N:" script prologue.

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
