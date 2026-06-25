#!/usr/bin/env bash
# v222 harness: function-def reconstruction edge cases — NESTED function defs
# render with a leading `function ` keyword (bash always adds it), while the
# OUTER named function stays keyword-free; and a brace-group function body with
# a redirect (`{ …; } 1>&2`) hoists its redirect onto the function's closing
# brace instead of double-wrapping. stdout/stderr is compared byte-for-byte
# between bash 5.2.21 (the reference) and huck. Requires bash on PATH.
set -u

_SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
HUCK_BIN="${HUCK_BIN:-$_SCRIPT_DIR/../../target/release/huck}"
if [[ ! -x "$HUCK_BIN" ]]; then
    echo "huck binary not found at $HUCK_BIN — run: cargo build --release --bin huck" >&2
    exit 1
fi
if ! command -v bash >/dev/null 2>&1; then
    echo "SKIP: bash not found on PATH; this differential harness requires bash" >&2
    exit 0
fi

PASS=0; FAIL=0
fragments=(
  'outer(){ echo a; function f3() { echo b; }; }; declare -f outer'
  'outer(){ echo a; f3() { echo b; }; }; declare -f outer'
  'outer(){ function g { echo b; }; }; type outer'
  'f(){ echo a; echo b; } 1>&2; declare -f f'
  'f4(){ echo a; f5() { echo b; } 1>&2; f5; } 2>&1; declare -f f4'
  'funcc() ( echo c ) 2>&1; declare -f funcc'
  'function topkw { echo a; }; declare -f topkw'
)

for frag in "${fragments[@]}"; do
    b_out=$(bash -c "$frag" 2>&1)
    h_out=$("$HUCK_BIN" -c "$frag" 2>&1)
    if [[ "$b_out" == "$h_out" ]]; then
        printf 'PASS: %s\n' "$frag"; PASS=$((PASS + 1))
    else
        printf 'FAIL: %s\n' "$frag"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
