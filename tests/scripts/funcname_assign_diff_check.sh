#!/usr/bin/env bash
# v223 harness: writes to FUNCNAME are silently discarded (rc 0, no error),
# matching bash 5.2.21 — `FUNCNAME=7`, `+=`, `for FUNCNAME`, and `read FUNCNAME`
# all leave $FUNCNAME reflecting the real call stack (empty at top level).
# Combined stdout+stderr is compared. Requires bash 5.2.x on PATH (the reference).
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
  'FUNCNAME=7; echo "[$FUNCNAME]"'
  'FUNCNAME=7; echo $?'
  'for FUNCNAME in x y; do :; done; echo "[$FUNCNAME]"'
  'read FUNCNAME <<< hello; echo "[$FUNCNAME]"'
  'f(){ FUNCNAME=x; echo "[$FUNCNAME]"; }; f'
  'f(){ echo "[$FUNCNAME]"; }; f; echo "[$FUNCNAME]"'
  'FUNCNAME+=z; echo "[$FUNCNAME]"'
)

for frag in "${fragments[@]}"; do
    b_out=$(bash --norc --noprofile -c "$frag" 2>&1)
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
