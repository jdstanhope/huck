#!/usr/bin/env bash
# Manual sanity check: run the same test-combinator fragments through
# bash and huck, diff outputs.
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
    '[ -n a -a -n b ]; echo $?'
    '[ -z a -o -n b ]; echo $?'
    '[ \( -n a -o -n b \) -a -n c ]; echo $?'
    '[ ! -n a ]; echo $?'
    '[ ! -n a -a -n b ]; echo $?'
    '[ \( -z "" -a -n x \) -o -n y ]; echo $?'
    '[ -a /tmp ]; echo $?'
    '[ -n a -a -n b -a -n c -a -n d ]; echo $?'
)

fail=0
for f in "${fragments[@]}"; do
    b_out=$(bash -c "$f" 2>&1)
    h_out=$(echo "$f" | "$HUCK" 2>&1)
    if [ "$b_out" != "$h_out" ]; then
        echo "DIFF on: $f"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") || true
        echo "---"
        fail=1
    fi
done

if [ "$fail" -eq 0 ]; then
    echo "all test-combinator fragments produce identical output to bash"
fi
exit "$fail"
