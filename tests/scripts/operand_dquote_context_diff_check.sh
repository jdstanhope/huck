#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v200 (M-15b): when a `${name<mod>WORD}`
# VALUE-substitution operand (`:-`/`:=`/`:?`/`:+` and the no-colon forms) is
# itself inside double quotes, the operand is in double-quote context — single
# quotes are LITERAL (kept) and backslash is special only before $ ` " \ .
# huck used to strip the single quotes and apply full unquoted escaping.
#
# PATTERN operands (`#`/`%`/`/`) are NOT value operands and keep their own
# quote-removal+glob-escape semantics; they're covered as regression guards.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1)
    h=$("$HUCK_BIN" -c "$frag" 2>&1)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# --- single quotes literal in VALUE operands inside dquote ---
check "single-q :-"      'echo "${x:-'\''a|b'\''}"'
check "single-q := "     'echo "${x:='\''d'\''}"'
check "single-q :+ set"  'x=v; echo "${x:+'\''a|b'\''}"'
check "single-q -"       'echo "${x-'\''a'\''}"'
check "single-q + set"   'x=v; echo "${x+'\''A'\''}"'
check "set shows value"  'x=hi; echo "${x:-'\''ignored'\''}"'
# --- backslash dquote-restricted in VALUE operands inside dquote ---
check "bslash star"      'echo "${x:-\*}"'        # \* -> \* (kept)
check "bslash dollar"    'echo "${x:-a\$b}"'      # \$ -> $ (dquote escape)
check "bslash n"         'echo "${x:-a\nb}"'      # \n -> \n (kept)
check "bslash dquote"    'echo "${x:-x\"z}"'      # \" -> " (dquote escape)
check "bslash backslash" 'echo "${x:-a\\b}"'      # \\ -> \ (dquote escape)
# --- UNQUOTED enclosing context: must stay quote-removed (unchanged) ---
check "unq single-q"     "echo \${x:-'a|b'}"
check "unq bslash star"  'echo ${x:-\*}'
check "unq bslash n"     'echo ${x:-a\nb}'
# --- VALUE operand with an expansion still expands (both contexts) ---
check "dq var in operand" 'y=YY; echo "${x:-$y}"'
check "unq var in operand" 'y=YY; echo ${x:-$y}'
# --- PATTERN operands: regression guards (must be unchanged) ---
check "pat # quoted"     'x=za; echo "${x#'\''z'\''}"'
check "pat % quoted"     'x=zab; echo "${x%'\''b'\''}"'
check "pat # noquote"    'x=aXb; echo "${x#a}"'
check "pat / literal"    'x=a.b; echo "${x/'\''.'\''/_}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
