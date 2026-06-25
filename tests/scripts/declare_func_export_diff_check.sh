#!/usr/bin/env bash
# v223 harness: `declare -F`/`-xF` function listing is byte-identical between
# bash 5.2.21 and huck. The names-only listing reflects the export attribute
# (`declare -fx NAME` for exported functions, `declare -f NAME` otherwise) and
# the `-x` flag filters the bulk listing to exported functions only. Combined
# stdout + exit status is compared. Requires bash 5.2.x on PATH (the reference).
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
  'a(){ :; }; b(){ :; }; declare -xF; echo END'
  'a(){ :; }; b(){ :; }; declare -xf; echo END'
  'a(){ :; }; zf(){ echo z; }; export -f zf; declare -F'
  'a(){ :; }; zf(){ echo z; }; export -f zf; declare -xF'
  'a(){ :; }; zf(){ echo z; }; export -f zf; declare -xf'
  'a(){ :; }; declare -F a'
  'a(){ :; }; zf(){ :; }; export -f zf; declare -F zf'
)

for frag in "${fragments[@]}"; do
    b_out=$(bash --norc --noprofile -c "$frag" 2>&1); b_rc=$?
    h_out=$("$HUCK_BIN" -c "$frag" 2>&1); h_rc=$?
    if [[ "$b_out" == "$h_out" && "$b_rc" == "$h_rc" ]]; then
        printf 'PASS: %s\n' "$frag"; PASS=$((PASS + 1))
    else
        printf 'FAIL: %s\n' "$frag"
        diff <(printf '%s\n[rc=%s]\n' "$b_out" "$b_rc") \
             <(printf '%s\n[rc=%s]\n' "$h_out" "$h_rc") | sed 's/^/    /'
        FAIL=$((FAIL + 1))
    fi
done

echo ""
echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
