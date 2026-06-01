#!/usr/bin/env bash
# Manual sanity check: run the same array fragments through bash and huck,
# diff outputs. Not part of `cargo test` (no bash dependency in CI), but
# run by the developer before merge.
set -u

HUCK="$(dirname "$0")/../../target/debug/huck"
if [ ! -x "$HUCK" ]; then
    echo "build huck first: cargo build" >&2
    exit 1
fi

fragments=(
    'a=(x y z); echo "${a[@]}"; echo "${#a[@]}"; echo "${!a[@]}"'
    'a=([5]=x [2]=y); echo "${#a[@]}"; echo "${!a[@]}"'
    'a=(x y z); for v in "${a[@]}"; do echo "[$v]"; done'
    'a=(x); a+=(y z); echo "${a[@]}"'
    'a[0]=hi; a[0]+=_bye; echo "${a[0]}"'
    'a=(a b c d); echo "${a[@]:1:2}"'
    'set -- one two three four; echo "${@:2:2}"'
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
    echo "all array fragments produce identical output to bash"
fi
exit "$fail"
