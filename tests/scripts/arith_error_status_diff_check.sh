#!/usr/bin/env bash
# v312 (#3/#49): an arithmetic-EXPANSION error DISCARDS the current top-level
# command — bash's jump_to_top_level(DISCARD): SECOND does NOT print and rc=1.
# This RESOLVED the old L-55 divergence (v215 had huck swallowing the error and
# continuing past it; now huck matches bash's `-c` discard). Substring index
# errors also abort (bash-identical). Compares STDOUT + EXIT CODE only (the error
# WORDING legitimately differs: `huck:` vs bash's text). Each "bad-arith" case is
# `<bad-arith>; echo SECOND`.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# For bash-identical cases: check byte-identical stdout + rc.
check_bash_identical() {
    local label="$1" frag="$2" b bo h ho
    bo=$(bash --norc --noprofile -c "$frag" 2>/dev/null); b=$?
    ho=$("$HUCK_BIN" -c "$frag" 2>/dev/null); h=$?
    if [[ "$bo" == "$ho" && "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  bash(rc=%s)=[%s]  huck(rc=%s)=[%s]\n' "$label" "$b" "$bo" "$h" "$ho"; FAIL=$((FAIL+1)); fi
}

# --- arithmetic expansion errors DISCARD the current command (bash-identical) ---
check_bash_identical "expansion semicolon"  'echo $((1;2)); echo SECOND'
check_bash_identical "expansion trailing +" 'echo $((1+)); echo SECOND'
check_bash_identical "expansion two terms"  'echo $((1 2)); echo SECOND'
check_bash_identical "assignment bad arith" 'x=$((1+)); echo SECOND'
check_bash_identical "arith with arr index" 'a=(x y); echo $((a[1+])); echo SECOND'
check_bash_identical "embedded in word"     'echo pre$((1 2))post; echo SECOND'
# --- substring index errors must abort (bash-identical) ---
check_bash_identical "substring offset+len" 'v=hello; echo ${v:1+:2}; echo SECOND'
check_bash_identical "substring offset only" 'v=hello; echo ${v:1+}; echo SECOND'
# --- controls: must NOT abort (bash-identical) ---
check_bash_identical "valid arith"          'echo $((1+2)); echo SECOND'
check_bash_identical "valid substring"      'v=hello; echo ${v:1:2}; echo SECOND'
check_bash_identical "standalone (( )) nonfatal" '(( 1+ )); echo SECOND'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
