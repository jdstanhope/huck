#!/usr/bin/env bash
# Manual sanity check: run the same IFS fragments through bash and huck,
# diff outputs. Not part of `cargo test` (no bash dependency in CI), but
# run by the developer before merge.
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
    'v="a b c"; for x in $v; do echo $x; done'
    'IFS=:; v="a:b:c"; for x in $v; do echo $x; done'
    'IFS=:; v="a::b"; for x in $v; do echo "[$x]"; done'
    'IFS=:; v=":a"; for x in $v; do echo "[$x]"; done'
    'IFS=:; v="a:"; for x in $v; do echo "[$x]"; done'
    'IFS=" :"; v="a : b"; for x in $v; do echo $x; done'
    'IFS=; v="a b c"; for x in $v; do echo "[$x]"; done'
    'set -- a b c; IFS=,; echo "$*"'
    'set -- a b c; IFS=; echo "$*"'
    'IFS=:; for x in $(echo "a:b:c"); do echo $x; done'
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
    echo "all IFS fragments produce identical output to bash"
fi
exit "$fail"
