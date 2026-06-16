#!/usr/bin/env bash
# v169 (L-24): a command substitution nested inside arithmetic must inherit the
# shell's extglob state. Run fragments through bash and huck; assert identical.
#
# NOTE: `shopt -s extglob` is on its OWN line (the whole logical line is lexed
# at once, so a same-line `shopt -s extglob; <use>` would not be on yet at lex
# time — that is the separate L-45 divergence, not L-24). The $'...\n...'
# entries embed that newline.
set -u

HUCK="$(dirname "$0")/../../target/debug/huck"
if [ ! -x "$HUCK" ]; then
    echo "build huck first: cargo build" >&2
    exit 1
fi
if ! command -v bash >/dev/null 2>&1; then
    echo "bash not found on PATH; this differential harness requires bash" >&2
    exit 1
fi

fragments=(
    # --- path A: $(( $(...extglob...) )) and backtick ---
    $'shopt -s extglob\necho $(( $( [[ foo == @(foo|bar) ]] && echo 1 || echo 0 ) ))'
    $'shopt -s extglob\necho $(( $( [[ z == !(a|b) ]] && echo 7 || echo 9 ) ))'
    $'shopt -s extglob\nv=$(( $( [[ ab == @(ab|cd) ]] && echo 5 || echo 6 ) )); echo $v'
    $'shopt -s extglob\necho $(( `[[ ab == @(ab|cd) ]] && echo 3 || echo 4` ))'
    # --- path B: (( )) standalone and for ((;;)) header ---
    $'shopt -s extglob\n(( $( [[ x == @(x|y) ]] && echo 1 || echo 0 ) )); echo $?'
    $'shopt -s extglob\nfor (( i=$( [[ a == @(a|b) ]] && echo 0 || echo 9 ); i<2; i++ )); do echo $i; done'
    # --- control: plain arith cmdsub (no extglob) is unchanged ---
    'echo $(( $(echo 3) + 4 ))'
    '(( $(echo 1) )); echo $?'
)

fail=0
for f in "${fragments[@]}"; do
    b_out=$(bash -c "$f" 2>&1)
    h_out=$("$HUCK" -c "$f" 2>&1)
    if [ "$b_out" != "$h_out" ]; then
        echo "DIFF on: $f"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") || true
        echo "---"
        fail=1
    fi
done

if [ "$fail" -eq 0 ]; then
    echo "all ${#fragments[@]} arith-extglob fragments produce identical output to bash"
fi
exit "$fail"
