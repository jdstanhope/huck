#!/usr/bin/env bash
# v218 harness: `declare -f` / `type` / `declare -F` function reconstruction is
# byte-identical between bash 5.2.21 and huck (print_cmd.c inside_function_def
# format). stdout is compared. Requires bash 5.2.x on PATH (the reference).
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
  'tf(){ echo a; echo b; }; declare -f tf'
  'tf(){ echo a; }; type tf'
  'f(){ ( exit 1 ); }; declare -f f'
  'f(){ ( a; b ); }; declare -f f'
  'f(){ { echo a; }; }; declare -f f'
  'f(){ a && b || c; }; declare -f f'
  'f(){ echo bg >/dev/null & echo next; }; declare -f f'
  'f(){ echo a | cat - >/dev/null; }; declare -f f'
  'f(){ if a; then b; fi; }; declare -f f'
  'f(){ if a; then b; elif c; then d; else e; fi; }; declare -f f'
  'f(){ while a; do b; done; }; declare -f f'
  'f(){ until a; do b; done; }; declare -f f'
  'f(){ while a; do b & done; }; declare -f f'
  'f(){ for x in 1 2; do echo $x; done; }; declare -f f'
  'f(){ for x; do echo $x; done; }; declare -f f'
  'f(){ for ((i=0; i<3; i++)); do echo $i; done; }; declare -f f'
  'f(){ select x in a b; do echo $x; done; }; declare -f f'
  'f(){ case $x in a) echo A;; b|c) echo BC;; esac; }; declare -f f'
  'f(){ (( i < 3 )); }; declare -f f'
  'f(){ i=$(( i + 1 )); }; declare -f f'
  'f(){ [[ -f x && $y == z ]]; }; declare -f f'
  'f(){ echo hi; }; declare -F f'
  'f(){ for ((i=0; i<3; i++)); do echo $i; done; }; type f'
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
