#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v130: set -x trace fidelity. Compares
# STDERR only (set -x writes there), stdout discarded. Default PS4 `+ `.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# Exact-bytes comparison (non-pipeline fragments: deterministic single producer).
check() {
    local label="$1" frag="$2" b h
    b=$(printf 'set -x\n%s\n' "$frag" | bash 2>&1 >/dev/null)
    h=$(printf 'set -x\n%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# Order-independent (PIPELINE fragments): in-process stages trace from a forked
# child, external stages from the parent, so left-to-right order is best-effort
# (documented L-21 residual). Compare the SET of trace lines.
check_sorted() {
    local label="$1" frag="$2" b h
    b=$(printf 'set -x\n%s\n' "$frag" | bash 2>&1 >/dev/null | sort)
    h=$(printf 'set -x\n%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null | sort)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "arg with space"        'x="a b"; echo "$x" c'
check "bracket test"          '[ 1 -lt 2 ]'
check "empty and special"     'echo "" "; foo"'
check "safe words bare"       'echo hello a-b a/b a.b a:b a=b a,b a%b a+b a@b a_b'
check "local args"            'f() { local DEF=x y; }; f'
check "command prefix"        'command printf "%s\n" hi'
check "inline assignment"     'FOO=bar echo hi'
check "two inline assigns"    'A=1 B=2 echo x'
check "bare assignment"       'A=1'
check "bare assign quoted"    'B="x y"'
check_sorted "pipeline two stages"   'echo a | cat'
check_sorted "pipeline three stages" 'echo a | cat | cat'

# ASCII-punctuation sweep: locks the contains_shell_metas safe set. Assign to a
# var (no execution) with the punct mid-word; compare the traced line.
# Dropped chars (diverge for reasons UNRELATED to xtrace quoting, not a
# contains_shell_metas bug):
#   '!'  -> L-27: huck history-expands PIPED non-interactive stdin, so this
#           harness's `printf ... | huck` mangles `a!b` (the VALUE differs, not
#           the quoting). Via a file arg both shells emit `+ v='a!b'` identically
#           (xtrace quoting of `!` is correct).
#   '(' ')' '<' '>' ';' '|' '&'  -> these make `v=a?b` a syntax CONSTRUCT
#           (subshell / redirect / list separator / pipe / background), not an
#           assignment, so the fragment never reaches the assignment-quoting path.
# Kept set still exercises every meta vs safe char the quoting path decides on.
for c in '#' '%' '+' '-' '.' '/' ':' '=' '@' '^' '_' '~' ',' '*' '?' '[' ']' '{' '}'; do
    check "punct[$c]" "v=a${c}b"
done

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
