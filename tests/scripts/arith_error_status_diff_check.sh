#!/usr/bin/env bash
# v215 harness: arithmetic-EXPANSION errors are non-fatal in huck (matching
# bash's script-file mode behavior). Substring index errors are fatal (matching
# bash). huck diverges from bash's -c mode behavior in continuing past arith
# errors (L-55 divergence documented in bash-divergences.md). Compares STDOUT +
# EXIT CODE only (the error WORDING legitimately differs: `huck:` vs bash's
# text). Each "bad-arith" case is `<bad-arith>; echo SECOND`: in huck (and
# bash file mode), SECOND prints and rc=0. Substring errors abort: SECOND
# doesn't print and rc=1.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# For arith errors: huck prints error + continues (matching bash file mode).
# bash -c aborts, so we check huck alone.
check_arith_nonfatal() {
    local label="$1" frag="$2" ho
    ho=$("$HUCK_BIN" -c "$frag" 2>/dev/null); h=$?
    # huck should print SECOND and return 0 (non-fatal arith error)
    if [[ "$ho" == *"SECOND"* && "$h" == 0 ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  huck(rc=%s)=[%s] (expected SECOND and rc=0)\n' "$label" "$h" "$ho"; FAIL=$((FAIL+1)); fi
}

# For bash-identical cases: check byte-identical stdout + rc.
check_bash_identical() {
    local label="$1" frag="$2" b bo h ho
    bo=$(bash --norc --noprofile -c "$frag" 2>/dev/null); b=$?
    ho=$("$HUCK_BIN" -c "$frag" 2>/dev/null); h=$?
    if [[ "$bo" == "$ho" && "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  bash(rc=%s)=[%s]  huck(rc=%s)=[%s]\n' "$label" "$b" "$bo" "$h" "$ho"; FAIL=$((FAIL+1)); fi
}

# --- arithmetic expansion errors are non-fatal in huck (L-55 divergence) ---
check_arith_nonfatal "expansion semicolon"  'echo $((1;2)); echo SECOND'
check_arith_nonfatal "expansion trailing +" 'echo $((1+)); echo SECOND'
check_arith_nonfatal "expansion two terms"  'echo $((1 2)); echo SECOND'
check_arith_nonfatal "assignment bad arith" 'x=$((1+)); echo SECOND'
check_arith_nonfatal "arith with arr index" 'a=(x y); echo $((a[1+])); echo SECOND'
check_arith_nonfatal "embedded in word"     'echo pre$((1 2))post; echo SECOND'
# --- substring index errors must abort (bash-identical) ---
check_bash_identical "substring offset+len" 'v=hello; echo ${v:1+:2}; echo SECOND'
check_bash_identical "substring offset only" 'v=hello; echo ${v:1+}; echo SECOND'
# --- controls: must NOT abort (bash-identical) ---
check_bash_identical "valid arith"          'echo $((1+2)); echo SECOND'
check_bash_identical "valid substring"      'v=hello; echo ${v:1:2}; echo SECOND'
check_bash_identical "standalone (( )) nonfatal" '(( 1+ )); echo SECOND'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
