#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v82 script-file mode + `-c`.
# `-c` fragments: compare `bash -c FRAG [name args...]` vs `huck -c FRAG [name args...]`.
# File fragment: write a temp script, run `bash FILE ARGS` vs `huck FILE ARGS`.
set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

# check_c LABEL FRAG [name [args...]]
# Run `bash -c FRAG [name args...]` and `huck -c FRAG [name args...]`, compare
# combined stdout+stderr and exit code.
check_c() {
    local label="$1"; shift
    local b h
    b=$(bash -c "$@" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$@" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then
        printf 'PASS: %s\n' "$label"
        PASS=$((PASS + 1))
    else
        printf 'FAIL: %s\n' "$label"
        diff <(printf '%s\n' "$b") <(printf '%s\n' "$h") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# 1. -c first-operand-is-$0 quirk + positionals
check_c "argv0 quirk"     'echo "0=$0 1=$1 #=$#"' name a b

# 2. no operands after -c CMD: $0 = shell name (bash = "bash"; huck = "huck")
#    Compare only $1 and $# so shell-name difference doesn't cause a mismatch.
check_c "no operands"     'echo "1=$1 #=$#"'

# 3. multi-statement / arithmetic
check_c "multi-statement" 'x=2; y=3; echo $((x+y))'

# 4. loop in -c
check_c "loop"            'for i in 1 2 3; do printf "%s," "$i"; done; echo'

# 5. exit code propagation
check_c "exit code"       'echo before; exit 7; echo after'

# 6. empty command string
check_c "empty command"   ''

# 7. File-mode fragment: identical script run by both shells.
#    The script uses basename "$0" so the full (but identical) path doesn't matter.
SCRIPT=$(mktemp)
printf 'echo "0=$(basename "$0") 1=$1 #=$#"\nfor w in "$@"; do printf "[%%s]" "$w"; done; echo\nexit 2\n' > "$SCRIPT"
bo=$(cd / && bash "$SCRIPT" a b 2>&1; echo "EXIT:$?")
ho=$(cd / && "$HUCK_BIN" "$SCRIPT" a b 2>&1; echo "EXIT:$?")
if [[ "$bo" == "$ho" ]]; then
    printf 'PASS: %s\n' "file mode"
    PASS=$((PASS + 1))
else
    printf 'FAIL: %s\n' "file mode"
    diff <(printf '%s\n' "$bo") <(printf '%s\n' "$ho") | sed 's/^/    /'
    FAIL=$((FAIL + 1))
fi
rm -f "$SCRIPT"

# 8. Missing-file invocation: both bash and huck exit 127 for a nonexistent
#    script. The stderr message text intentionally differs (bash uses its own
#    prefix; huck uses "huck:" — documented in M-77a), so we compare ONLY the
#    exit code here, not stderr.
bc=$(bash /no/such/script-xyz 2>/dev/null; echo "$?")
hc=$("$HUCK_BIN" /no/such/script-xyz 2>/dev/null; echo "$?")
if [[ "$bc" == "$hc" ]]; then
    printf 'PASS: %s\n' "missing file exit code ($bc)"
    PASS=$((PASS + 1))
else
    printf 'FAIL: %s\n' "missing file exit code"
    printf '    bash=%s huck=%s\n' "$bc" "$hc"
    FAIL=$((FAIL + 1))
fi

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
