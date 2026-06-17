#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v178: an arithmetic-EXPANSION error
# ($((…)), substring index ${v:off:len}) is fatal — the command aborts with a
# nonzero status and the rest of the list does not run, matching bash. Compares
# STDOUT + EXIT CODE only (the error WORDING legitimately differs: `huck:` vs
# bash's text). Each "bad" case is `<bad-arith>; echo SECOND`: if the arith error
# aborts, SECOND never prints.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b bo h ho
    bo=$(bash --norc --noprofile -c "$frag" 2>/dev/null); b=$?
    ho=$("$HUCK_BIN" -c "$frag" 2>/dev/null); h=$?
    if [[ "$bo" == "$ho" && "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  bash(rc=%s)=[%s]  huck(rc=%s)=[%s]\n' "$label" "$b" "$bo" "$h" "$ho"; FAIL=$((FAIL+1)); fi
}

# --- arithmetic expansion errors must abort (empty stdout, rc 1, no SECOND) ---
check "expansion semicolon"  'echo $((1;2)); echo SECOND'
check "expansion trailing +" 'echo $((1+)); echo SECOND'
check "expansion two terms"  'echo $((1 2)); echo SECOND'
check "assignment bad arith" 'x=$((1+)); echo SECOND'
check "arith with arr index" 'a=(x y); echo $((a[1+])); echo SECOND'
check "embedded in word"     'echo pre$((1 2))post; echo SECOND'
# --- substring index errors must abort ---
check "substring offset+len" 'v=hello; echo ${v:1+:2}; echo SECOND'
check "substring offset only" 'v=hello; echo ${v:1+}; echo SECOND'
# --- controls: must NOT abort ---
check "valid arith"          'echo $((1+2)); echo SECOND'
check "valid substring"      'v=hello; echo ${v:1:2}; echo SECOND'
check "standalone (( )) nonfatal" '(( 1+ )); echo SECOND'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
