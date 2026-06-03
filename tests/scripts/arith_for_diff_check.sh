#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for the C-style arith for-loop
# `for ((init;cond;step)) do BODY done` and the standalone `((expr))`
# command. Each fragment runs through `bash` and `huck` via stdin
# (huck has no -c flag); outputs must be byte-identical.

set -u

HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run cargo build first" >&2
    exit 1
fi

PASS=0
FAIL=0

check() {
    local label="$1"
    local fragment="$2"
    local bash_out huck_out

    bash_out=$(printf '%s\n' "$fragment" | bash 2>&1; echo "EXIT:$?")
    huck_out=$(printf '%s\n' "$fragment" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")

    if [[ "$bash_out" == "$huck_out" ]]; then
        printf "PASS: %s\n" "$label"
        PASS=$((PASS + 1))
    else
        printf "FAIL: %s\n" "$label"
        diff <(echo "$bash_out") <(echo "$huck_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
}

# 1. Standalone arith with non-zero result.
check "((1+2)) exit code" \
      '((1+2)); echo $?'

# 2. Standalone arith with zero result.
check "((0)) exit code" \
      '((0)); echo $?'

# 3. Counter loop.
check "for counter" \
      'for ((i=0;i<3;i++)); do echo $i; done'

# 4. Infinite loop with break.
check "for empty header break" \
      'for ((;;)); do break; done; echo ok'

# 5. Continue with step.
check "for continue" \
      'for ((i=0;i<5;i++)); do if [ $i -eq 2 ]; then continue; fi; echo $i; done'

# 6. Arith in if-condition.
check "if arith condition" \
      'if ((5 > 3)); then echo yes; fi'

# 7. Post-increment side effect.
check "post-increment" \
      'x=10; ((x++)); echo $x'

# 8. Nested arith-for loops.
check "nested arith-for" \
      'for ((i=0;i<2;i++)); do for ((j=0;j<2;j++)); do printf "%d%d " $i $j; done; done; echo'

# 9. Zero-result side-effect (assignment of zero exits 1).
check "((x=0)) is exit 1" \
      '((x=0)); echo $?'

# 10. Cond evaluated each iteration (mutable cond).
check "cond re-evaluated" \
      'x=3; for ((;x>0;x--)); do echo $x; done'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
