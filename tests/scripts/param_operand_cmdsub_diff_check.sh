#!/usr/bin/env bash
# G1/v270: a command substitution / arithmetic expansion / backtick nested
# inside a DOUBLE-QUOTED span within a ${…} modifier operand. bash accepts all
# of these; huck used to error "syntax error: unsupported expansion" (the old
# DeferredExpansion reject). Run each fragment through bash and huck and assert
# byte-identical output, incl. the quoted-operand word-split semantics (a
# quoted operand cmdsub is ONE field; an unquoted one is split).
set -u

HUCK="$(dirname "$0")/../../target/debug/huck"
if [ ! -x "$HUCK" ]; then
    echo "build huck first: cargo build -p huck" >&2
    exit 1
fi
if ! command -v bash >/dev/null 2>&1; then
    echo "bash not found on PATH; this differential harness requires bash" >&2
    exit 1
fi

fragments=(
    # --- value families: :- := :? :+ and non-colon - = + (first char) ---
    'x=abc; echo "${x:-"$(echo D)"}"'
    'unset x; echo "${x:-"$(echo D)"}"'
    'unset x; echo "${x:="$(echo D)"}"; echo "$x"'
    'x=abc; echo "${x:+"$(echo D)"}"'
    'unset x; echo "${x-"$(echo D)"}"'
    'unset x; echo "${x="$(echo D)"}"'
    'x=abc; echo "${x+"$(echo D)"}"'
    # --- value families: mid-span and multiple expansions ---
    'unset x; echo "${x:-"a$(echo b)c"}"'
    'unset x; echo "${x:-"$(echo a)$(echo b)"}"'
    # --- pattern removal: # ## % %% ---
    'x=abcabc; echo "${x#"$(echo a)"}"'
    'x=abcabc; echo "${x##"$(echo a)bc"}"'
    'x=abcabc; echo "${x%"$(echo c)"}"'
    'x=abcabc; echo "${x%%"a$(echo bc)"}"'
    # --- substitute: / // /# /% (replacement segment in dquotes) ---
    'x=abc; echo "${x/b/"$(echo Z)"}"'
    'x=abc; echo "${x/b/"x$(echo Z)y"}"'
    'x=aaa; echo "${x//a/"$((7+1))"}"'
    'x=abc; echo "${x/#a/"$(echo Z)"}"'
    'x=abc; echo "${x/%c/"$(echo Z)"}"'
    # --- pattern segment (first) of a substitute in dquotes ---
    'x=abc; echo "${x/"$(echo b)"/Z}"'
    # --- case: ^ ^^ , ,, (pattern operand in dquotes) ---
    'x=abc; echo "${x^"$(echo a)"}"'
    'x=abc; echo "${x^^"$(echo abc)"}"'
    'x=ABC; echo "${x,"$(echo A)"}"'
    'x=ABC; echo "${x,,"$(echo ABC)"}"'
    # --- substring offset/length operands in dquotes ---
    'x=abcdef; echo "${x:"$((1+1))"}"'
    'x=abcdef; echo "${x:"$((1+1))":"$((2))"}"'
    # --- arithmetic and backtick siblings (first char + mid) ---
    'unset x; echo "${x:-"$((3*4))"}"'
    'unset x; echo "${x:-"q$((3*4))r"}"'
    'unset x; echo "${x:-"`echo BT`"}"'
    'unset x; echo "${x:-"a`echo BT`b"}"'
    # --- nested operand-in-dquote ---
    'unset x y; echo "${x:-"${y:-"$(echo N)"}"}"'
    # --- word-split semantics: quoted operand cmdsub = ONE field ---
    'unset y; printf "[%s]" ${y:-"$(printf "a b")"}; echo'
    'unset y; printf "[%s]" ${y:-$(printf "a b")}; echo'
    'unset y; printf "[%s]" ${y:-x"a`printf "c d"`"}; echo'
    'unset y; printf "[%s]" ${y:-"$((1+2)) $((3+4))"}; echo'
    # --- outer double-quote wrapping the whole ${…} ---
    'x=abc; echo "${x#"$(echo a)"}"'
    ': "${z:="v$(echo w)x"}"; echo "$z"'
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
    echo "all ${#fragments[@]} param-operand-cmdsub fragments produce identical output to bash"
fi
exit "$fail"
