#!/usr/bin/env bash
# Byte-identical bash↔huck diff harness for the `function NAME { ... }`
# keyword form. Each fragment runs through `bash` and `huck` via stdin
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

# 1. Basic keyword-form definition + call.
check "function-keyword brace body" \
      'function greet { echo hello; }; greet'

# 2. Keyword form with optional parens.
check "function-keyword with parens" \
      'function greet() { echo hi; }; greet'

# 3. Keyword form with subshell body.
check "function-keyword subshell body" \
      'function f () ( echo nested ); f'

# 4. Keyword form with positional args.
check "function-keyword positional args" \
      'function f { echo "$1-$2"; }; f alpha beta'

# 5. Keyword form with if body (no braces).
check "function-keyword if body" \
      'function f if true; then echo cond; fi; f'

# 6. Keyword and POSIX forms produce identical behavior.
check "function-keyword vs POSIX equivalence" \
      'function kf { echo via-$1; }; pf() { echo via-$1; }; kf x; pf y'

echo ""
echo "Total: $((PASS + FAIL)), Pass: $PASS, Fail: $FAIL"
exit $((FAIL > 0 ? 1 : 0))
