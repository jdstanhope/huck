#!/usr/bin/env bash
# v165: run ${…} substitution/substring operands containing command
# substitutions through bash and huck, asserting byte-identical output.
# Guards the L-10 fix ($()/backtick/$(( )) delimiters skipped) and that the
# plain (no-command-substitution) forms are unchanged.
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
    # --- L-10 cases: delimiter inside $(...) ---
    's=abcdefgh; echo "${s:$(echo 1:2 | cut -d: -f1)}"'
    's=a-b-c; echo "${s/$(echo a/x)/Z}"'
    's=a.b.c; echo "${s/$(echo .)/X}"'
    # --- backtick command substitution ---
    's=a-b; echo "${s/`echo a/x`/Z}"'
    # --- $(( )) arithmetic operands, incl. a ternary colon ---
    's=abcdef; echo "${s:$((1+1)):$((1+2))}"'
    's=abcdef; echo "${s:$((1>0?2:3))}"'
    # --- nested $( $() ) ---
    's=xyz; echo "${s/$(echo $(echo a/b))/Q}"'
    # --- quoted / escaped delimiter in the operand ---
    's=axb; echo "${s/"a/b"/Z}"'
    's=a/b/c; echo "${s/a\/b/Z}"'
    # --- L-52: literal } inside a command substitution in the operand ---
    's=a}b; echo "${s/$(echo a}b)/Z}"'
    's=xy; echo "${s/`echo a}b`/Z}"'
    's=xyz; echo "${s/$(echo $(echo a}b))/Q}"'
    's=ab; echo "${s/$(echo "}")/Z}"'
    # --- plain forms (must be unchanged by the refactor) ---
    's=abcdefgh; echo "${s:2:3}"'
    's=abcdefgh; echo "${s:2}"'
    's=a.b.c; echo "${s//./X}"'
    's=a.b.c; echo "${s/./X}"'
    's=hello; echo "${s/l}"'
    's=foobar; echo "${s#foo}"; echo "${s%bar}"'
    's=abc; echo "${s:${#s}-1}"'
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
    echo "all ${#fragments[@]} param-cmdsub-split fragments produce identical output to bash"
fi
exit "$fail"
