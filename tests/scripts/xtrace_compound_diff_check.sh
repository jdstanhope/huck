#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v198: set -x traces of compound commands.
# Combined stderr (where the trace goes); $PS4 default `+ ` is identical in both,
# so no normalization is needed for these cases.
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
# --- case ---
check "case match"     'set -x; x=hi; case "$x" in hi) :;; esac'
check "case no-match"  'set -x; case z in a) :;; esac'
check "case modifier"  'set -x; p=a/b/c; case ${p##*/} in c) :;; esac'
# --- for-in (per iteration) ---
check "for literal"    'set -x; for i in a b c; do :; done'
check "for raw var"    'set -x; xs="a b"; for i in $xs; do :; done'
check "for quoted"     'set -x; for i in a "b c"; do :; done'
check "for empty list" 'set -x; for i in; do :; done; echo done'
check "for cmdsub"     'set -x; for i in $(echo p q); do :; done'
# --- select ---
check "select"         'set -x; select x in a b; do break; done <<< 1'
# --- standalone (( )) ---
check "arith simple"   'set -x; ((1+1))'
check "arith spaces"   'set -x; (( v=3, v+1 ))'
# --- C-style for ---
check "c-for"          'set -x; for ((i=0;i<2;i++)); do :; done'
# --- while/if regression (no own header; condition traces) ---
check "while cond"     'set -x; n=0; while (( n < 2 )); do (( n++ )); done'
check "if cond"        'set -x; if [[ 1 == 1 ]]; then :; fi'
# --- [[ ]] leaf-by-leaf ---
check "dbracket single"   'set -x; v=5; [[ $v -gt 3 ]]'
check "dbracket regex"    'set -x; [[ "" =~ ^[0-9]+$ ]]'
check "dbracket and"      'set -x; a=1;b=2; [[ $a == 1 && $b == 2 ]]'
check "dbracket or-short" 'set -x; a=1;b=2; [[ $a == 1 || $b == 9 ]]'
check "dbracket and-fail" 'set -x; a=1;b=2; [[ $a == 9 && $b == 2 ]]'
check "dbracket not"      'set -x; [[ ! -e /nonesuch ]]'
check "dbracket parens"   'set -x; [[ ( 1 == 1 ) ]]'
check "dbracket glob"     'set -x; [[ hi == h* ]]'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
